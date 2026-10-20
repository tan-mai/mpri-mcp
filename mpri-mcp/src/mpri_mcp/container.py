"""Wires the long-lived components together; shared by the web app and the CLI."""

from __future__ import annotations

from mpri_mcp.ai.registry import ProviderRegistry
from mpri_mcp.auth.oauth_provider import SqliteOAuthProvider
from mpri_mcp.config import Settings
from mpri_mcp.crawler.browser import BrowserManager
from mpri_mcp.crawler.extractor import PriceExtractor
from mpri_mcp.crawler.runner import CrawlRunner
from mpri_mcp.db import Database
from mpri_mcp.scheduler.backup import backup_sqlite
from mpri_mcp.scheduler.engine import SchedulerEngine


class Container:
    def __init__(
        self,
        settings: Settings,
        *,
        registry: ProviderRegistry | None = None,
        browser: BrowserManager | None = None,
    ) -> None:
        self.settings = settings
        self.db = Database(settings.resolved_database_url)
        self.registry = registry or ProviderRegistry(settings)
        self.browser = browser or BrowserManager(settings)
        self.extractor = PriceExtractor(self.registry)
        self.runner = CrawlRunner(settings, self.db, self.browser, self.extractor, self.registry)
        self.oauth = SqliteOAuthProvider(settings, self.db)
        self.scheduler = SchedulerEngine(
            settings, self.runner.crawl_supermarket, self._housekeeping
        )

    async def _housekeeping(self) -> None:
        """Daily: back up SQLite and purge expired OAuth codes/tokens."""
        await self.oauth.cleanup_expired()
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
        await self.browser.stop()
        await self.db.dispose()
