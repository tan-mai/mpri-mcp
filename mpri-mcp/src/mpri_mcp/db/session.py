"""Async engine/session management for SQLite (WAL mode, foreign keys on), plus the startup
step that resets a database left by the server-crawler release (once, after a backup)."""

from __future__ import annotations

import asyncio
import logging
import sqlite3
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any

from sqlalchemy import Connection, event, inspect, text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from mpri_mcp.db.models import Base
from mpri_mcp.utils.time import utcnow

log = logging.getLogger(__name__)

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
# Tables of the server-crawler release that hold business data. On the first start of the
# agent-crawl release they are dropped (children first) and created again, empty. The admin
# account and OAuth tables are kept, so the dashboard login and Claude's connector still work.
LEGACY_DATA_TABLES = (
    "crawl_run_items",
    "crawl_runs",
    "price_records",
    "product_urls",
    "products",
    "supermarkets",
    "app_settings",
)


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
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        async with self.engine.begin() as conn:
            legacy = await conn.run_sync(_is_server_crawler_db)
        if legacy:
            await self._reset_legacy_data()
        async with self.engine.begin() as conn:
            await conn.run_sync(_check_schema)
            await conn.run_sync(Base.metadata.create_all)

    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    async def _reset_legacy_data(self) -> None:
        """Back up the whole database, then drop the old business tables (created again,
        empty, by create_all). Runs once: the new tables no longer have crawl_config."""
        backup = None
        if self.sqlite_path is not None and self.sqlite_path.exists():
            backup = await asyncio.to_thread(_backup_legacy, self.sqlite_path)
        async with self.engine.begin() as conn:
            for table in LEGACY_DATA_TABLES:
                await conn.execute(text(f"DROP TABLE IF EXISTS {table}"))
        log.warning(
            "Database from the server-crawler release found: backed up to %s, then reset. "
            "Supermarkets, products, prices and crawl history were removed; the admin "
            "account and Claude's connections were kept.",
            backup or "(no file to back up)",
        )

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


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def _is_server_crawler_db(conn: Connection) -> bool:
    """The server-crawler release stored crawl_config on supermarkets; this one never does."""
    inspector = inspect(conn)
    if "supermarkets" not in inspector.get_table_names():
        return False
    return "crawl_config" in {c["name"] for c in inspector.get_columns("supermarkets")}


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def _backup_legacy(db_path: Path) -> Path:
    """Full copy via SQLite's backup API, outside the rotation of the daily mpri-*.db backups."""
    backup_dir = db_path.parent / "backups"
    backup_dir.mkdir(parents=True, exist_ok=True)
    dest = backup_dir / f"legacy-before-agent-crawl-{utcnow():%Y%m%d-%H%M%S}.db"
    src = sqlite3.connect(db_path)
    dst = sqlite3.connect(dest)
    try:
        with dst:
            src.backup(dst)
    finally:
        dst.close()
        src.close()
    return dest


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def _check_schema(conn: Connection) -> None:
    """Any other mismatch between the models and an existing table would fail on the first
    query, so stop at startup with a clear message instead of guessing what to delete."""
    inspector = inspect(conn)
    existing_tables = set(inspector.get_table_names())
    for name, table in Base.metadata.tables.items():
        if name not in existing_tables:
            continue
        existing = {c["name"] for c in inspector.get_columns(name)}
        missing = sorted(c.name for c in table.columns if c.name not in existing)
        if missing:
            raise RuntimeError(
                f"The database at {conn.engine.url.database} does not match this version "
                f"(table '{name}' has no column {', '.join(missing)})."
            )


def _sqlite_pragmas(dbapi_conn: Any, _record: Any) -> None:
    cursor = dbapi_conn.cursor()
    cursor.execute("PRAGMA journal_mode=WAL")
    cursor.execute("PRAGMA foreign_keys=ON")
    cursor.execute("PRAGMA busy_timeout=10000")
    cursor.execute("PRAGMA synchronous=NORMAL")
    cursor.close()
