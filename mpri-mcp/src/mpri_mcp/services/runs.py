"""Crawl run history (read side, for the dashboard)."""

from __future__ import annotations

from sqlalchemy import select
from sqlalchemy.orm import selectinload

from mpri_mcp.db import Database
from mpri_mcp.db.models import CrawlRun, CrawlRunItem, ProductUrl
from mpri_mcp.i18n import _
from mpri_mcp.services.errors import NotFoundError


async def recent_runs(
    db: Database, limit: int = 50, supermarket_id: str | None = None
) -> list[CrawlRun]:
    stmt = select(CrawlRun).order_by(CrawlRun.id.desc()).limit(limit)
    if supermarket_id:
        stmt = stmt.where(CrawlRun.supermarket_id == supermarket_id)
    async with db.session() as s:
        return list((await s.scalars(stmt)).all())


async def get_run(db: Database, run_id: int) -> tuple[CrawlRun, list[CrawlRunItem]]:
    async with db.session() as s:
        run = await s.get(CrawlRun, run_id)
        if run is None:
            raise NotFoundError(_("Run #{id} not found.").format(id=run_id))
        items = (
            await s.scalars(
                select(CrawlRunItem)
                .where(CrawlRunItem.run_id == run_id)
                .order_by(CrawlRunItem.status.desc(), CrawlRunItem.id)
            )
        ).all()
    return run, list(items)


async def failing_urls(db: Database, limit: int = 50) -> list[ProductUrl]:
    async with db.session() as s:
        return list(
            (
                await s.scalars(
                    select(ProductUrl)
                    .where(ProductUrl.is_active, ProductUrl.consecutive_failures > 0)
                    .order_by(ProductUrl.consecutive_failures.desc())
                    .limit(limit)
                    .options(selectinload(ProductUrl.product), selectinload(ProductUrl.supermarket))
                )
            ).all()
        )
