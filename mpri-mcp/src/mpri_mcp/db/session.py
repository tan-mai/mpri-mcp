"""Async engine/session management for SQLite (WAL mode, foreign keys on), plus the
add-column step that upgrades existing databases at startup."""

from __future__ import annotations

import logging
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any

from sqlalchemy import Connection, event, inspect, text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.schema import CreateColumn

from mpri_mcp.db.models import Base

log = logging.getLogger(__name__)

# Columns added to existing tables after the first release. create_all() only creates missing
# tables, so these are added with ALTER TABLE at startup. Each needs a server default (or must
# be nullable) so existing rows — and older images rolled back onto this schema — keep working.
ADDED_COLUMNS: dict[str, tuple[str, ...]] = {
    "supermarkets": ("wait_for_api", "api_url_contains"),
}


class Database:
    def __init__(self, url: str) -> None:
        self.url = url
        parsed = make_url(url)
        self.is_sqlite = parsed.get_backend_name() == "sqlite"
        if self.is_sqlite and parsed.database and parsed.database != ":memory:":
            Path(parsed.database).parent.mkdir(parents=True, exist_ok=True)
        self.sqlite_path = Path(parsed.database) if self.is_sqlite and parsed.database else None

        connect_args: dict[str, Any] = {"timeout": 30} if self.is_sqlite else {}
        self.engine = create_async_engine(url, connect_args=connect_args)
        if self.is_sqlite:
            event.listen(self.engine.sync_engine, "connect", _sqlite_pragmas)
        self._sessionmaker = async_sessionmaker(self.engine, expire_on_commit=False)

    async def create_all(self) -> None:
        async with self.engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all)
            await conn.run_sync(_add_missing_columns)

    @asynccontextmanager
    async def session(self) -> AsyncIterator[AsyncSession]:
        """A session that commits on success and rolls back on error."""
        async with self._sessionmaker() as session:
            try:
                yield session
                await session.commit()
            except BaseException:
                await session.rollback()
                raise

    async def dispose(self) -> None:
        await self.engine.dispose()


def _add_missing_columns(conn: Connection) -> None:
    inspector = inspect(conn)
    for table_name, names in ADDED_COLUMNS.items():
        existing = {c["name"] for c in inspector.get_columns(table_name)}
        table = Base.metadata.tables[table_name]
        for name in names:
            if name in existing:
                continue
            column_ddl = CreateColumn(table.c[name]).compile(dialect=conn.dialect)
            conn.execute(text(f"ALTER TABLE {table_name} ADD COLUMN {column_ddl}"))
            log.info("Added column %s.%s", table_name, name)


def _sqlite_pragmas(dbapi_conn: Any, _record: Any) -> None:
    cursor = dbapi_conn.cursor()
    cursor.execute("PRAGMA journal_mode=WAL")
    cursor.execute("PRAGMA foreign_keys=ON")
    cursor.execute("PRAGMA busy_timeout=10000")
    cursor.execute("PRAGMA synchronous=NORMAL")
    cursor.close()
