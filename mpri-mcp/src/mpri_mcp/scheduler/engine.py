"""In-process scheduler: one independent cron job per supermarket, plus housekeeping jobs."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Awaitable, Callable
from datetime import datetime
from typing import Any
from zoneinfo import ZoneInfo

from apscheduler.jobstores.base import JobLookupError
from apscheduler.schedulers.asyncio import AsyncIOScheduler
from apscheduler.triggers.cron import CronTrigger
from sqlalchemy import select

from mpri_mcp.config import Settings
from mpri_mcp.db import Database
from mpri_mcp.db.models import Supermarket
from mpri_mcp.scheduler.cron import CronError, build_trigger

log = logging.getLogger(__name__)

CrawlFunc = Callable[[str, str], Awaitable[Any]]  # (supermarket_id, trigger) -> ...
BackupFunc = Callable[[], Awaitable[Any]]


def market_job_id(supermarket_id: str) -> str:
    return f"crawl:{supermarket_id}"


class SchedulerEngine:
    def __init__(
        self,
        settings: Settings,
        crawl_func: CrawlFunc,
        backup_func: BackupFunc | None = None,
    ) -> None:
        self._settings = settings
        self._crawl = crawl_func
        self._backup = backup_func
        self._scheduler = AsyncIOScheduler(
            timezone=ZoneInfo(settings.scheduler_timezone),
            job_defaults={
                "coalesce": True,
                "max_instances": 1,
                "misfire_grace_time": settings.misfire_grace_seconds,
            },
        )
        self._running: set[str] = set()
        self._tasks: set[asyncio.Task[Any]] = set()

    # ------------------------------------------------------------------ lifecycle
    async def start(self, db: Database) -> None:
        self._scheduler.start()
        await self.sync_from_db(db)
        if self._backup is not None:
            try:
                trigger = build_trigger(
                    self._settings.backup_cron, self._settings.scheduler_timezone
                )
            except CronError as e:
                log.error(
                    "Invalid BACKUP_CRON %r: %s — backups disabled", self._settings.backup_cron, e
                )
            else:
                self._scheduler.add_job(
                    self._run_backup,
                    trigger=trigger,
                    id="backup",
                    name="daily housekeeping",
                    replace_existing=True,
                )

    async def shutdown(self, grace_seconds: float = 10) -> None:
        if self._scheduler.running:
            self._scheduler.shutdown(wait=False)
        tasks = list(self._tasks)
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.wait(tasks, timeout=grace_seconds)

    async def sync_from_db(self, db: Database) -> None:
        async with db.session() as s:
            markets = (await s.scalars(select(Supermarket))).all()
        wanted = set()
        for m in markets:
            if m.cron_enabled and m.cron_expression:
                try:
                    self.upsert_market_job(m.id, m.cron_expression, m.timezone)
                    wanted.add(market_job_id(m.id))
                except CronError as e:
                    log.error("Skipping schedule for %s: %s", m.id, e)
        for job in self._scheduler.get_jobs():
            if job.id.startswith("crawl:") and job.id not in wanted:
                job.remove()
        log.info("Scheduler loaded %d market job(s)", len(wanted))

    # ------------------------------------------------------------------ market jobs
    def upsert_market_job(self, supermarket_id: str, expression: str, timezone: str) -> CronTrigger:
        trigger = build_trigger(expression, timezone)
        self._scheduler.add_job(
            self._run_market,
            trigger=trigger,
            id=market_job_id(supermarket_id),
            name=f"crawl {supermarket_id}",
            args=[supermarket_id, "cron"],
            replace_existing=True,
        )
        return trigger

    def remove_market_job(self, supermarket_id: str) -> None:
        try:
            self._scheduler.remove_job(market_job_id(supermarket_id))
        except JobLookupError:
            pass

    def next_run_time(self, supermarket_id: str) -> datetime | None:
        job = self._scheduler.get_job(market_job_id(supermarket_id))
        return job.next_run_time if job else None

    def is_running(self, supermarket_id: str) -> bool:
        return supermarket_id in self._running

    def run_now(self, supermarket_id: str) -> bool:
        """Start a crawl in the background. False if that market is already crawling."""
        if supermarket_id in self._running:
            return False
        self._running.add(supermarket_id)  # claim now so a second click can't double-start
        task = asyncio.create_task(self._run_market(supermarket_id, "manual", claimed=True))
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
        return True

    async def _run_market(self, supermarket_id: str, trigger: str, claimed: bool = False) -> None:
        if not claimed:
            if supermarket_id in self._running:
                log.warning(
                    "Crawl of %s already running; skipping %s trigger", supermarket_id, trigger
                )
                return
            self._running.add(supermarket_id)
        task = asyncio.current_task()
        if task is not None:
            self._tasks.add(task)
        try:
            await self._crawl(supermarket_id, trigger)
        except asyncio.CancelledError:
            log.warning("Crawl of %s cancelled", supermarket_id)
            raise
        except Exception:
            log.exception("Crawl of %s failed", supermarket_id)
        finally:
            self._running.discard(supermarket_id)
            if task is not None:
                self._tasks.discard(task)

    async def _run_backup(self) -> None:
        assert self._backup is not None
        try:
            await self._backup()
        except Exception:
            log.exception("Backup failed")
