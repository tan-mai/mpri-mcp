"""Wires the long-lived components together; shared by the web app and the CLI."""

from __future__ import annotations

from mpri_mcp.auth.oauth_provider import SqliteOAuthProvider
from mpri_mcp.config import Settings
from mpri_mcp.db import Database
from mpri_mcp.scheduler.backup import backup_sqlite
from mpri_mcp.scheduler.engine import SchedulerEngine
from mpri_mcp.services import agent_tasks


class Container:
    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    def __init__(self, settings: Settings) -> None:
        self.settings = settings
        self.db = Database(settings.resolved_database_url)
        self.oauth = SqliteOAuthProvider(settings, self.db)
        # The server crawls nothing itself: each supermarket's cron job queues crawl tasks
        # for Claude (see services/agent_tasks.py).
        self.scheduler = SchedulerEngine(settings, self._queue_market, self._housekeeping)

    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    async def _queue_market(self, supermarket_id: str, trigger: str) -> None:
        await agent_tasks.queue_market(self.db, self.settings, supermarket_id, trigger)

    async def _housekeeping(self) -> None:
        """Daily: back up SQLite, purge expired OAuth codes/tokens, expire stale crawl tasks."""
        await self.oauth.cleanup_expired()
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        await agent_tasks.maintain(self.db, self.settings)
        if self.db.sqlite_path is not None:
            await backup_sqlite(
                self.db.sqlite_path, self.settings.data_dir / "backups", self.settings.backup_keep
            )

    async def start(self, *, scheduler: bool = True) -> None:
        self.settings.data_dir.mkdir(parents=True, exist_ok=True)
        await self.db.create_all()
        if scheduler:
            await self.scheduler.start(self.db)

    async def stop(self) -> None:
        await self.scheduler.shutdown()
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        await self.db.dispose()
