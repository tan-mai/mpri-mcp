"""Schedule management: keeps the supermarkets table and the live scheduler in sync."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime

from sqlalchemy import func, select

from mpri_mcp.db import Database
from mpri_mcp.db.models import ProductUrl, Supermarket
from mpri_mcp.scheduler.cron import CronError, build_trigger, next_fire_times, normalize_expression
from mpri_mcp.scheduler.engine import SchedulerEngine
from mpri_mcp.services.errors import ValidationError
from mpri_mcp.services.supermarkets import get_supermarket, set_schedule_fields


@dataclass
class ScheduleResult:
    supermarket: Supermarket
    cron_expression: str
    timezone: str
    next_runs: list[datetime]


def preview(expression: str, timezone: str, count: int = 3) -> list[datetime]:
    try:
        return next_fire_times(build_trigger(expression, timezone), count)
    except CronError as e:
        raise ValidationError(f"Invalid cron expression: {e}") from e


async def set_cron(
    db: Database, engine: SchedulerEngine, supermarket_id: str, cron_expression: str
) -> ScheduleResult:
    market = await get_supermarket(db, supermarket_id)
    expression = normalize_expression(cron_expression)
    next_runs = preview(expression, market.timezone)
    market = await set_schedule_fields(
        db, supermarket_id, cron_expression=expression, cron_enabled=True
    )
    engine.upsert_market_job(supermarket_id, expression, market.timezone)
    return ScheduleResult(market, expression, market.timezone, next_runs)


async def disable_cron(db: Database, engine: SchedulerEngine, supermarket_id: str) -> Supermarket:
    market = await get_supermarket(db, supermarket_id)
    market = await set_schedule_fields(
        db, supermarket_id, cron_expression=market.cron_expression, cron_enabled=False
    )
    engine.remove_market_job(supermarket_id)
    return market


async def trigger_now(db: Database, engine: SchedulerEngine, supermarket_id: str) -> bool:
    """Returns False if a crawl of that market is already in progress."""
    await get_supermarket(db, supermarket_id)
    async with db.session() as s:
        active = await s.scalar(
            select(func.count())
            .select_from(ProductUrl)
            .where(ProductUrl.supermarket_id == supermarket_id, ProductUrl.is_active)
        )
    if not active:
        raise ValidationError(
            f"Supermarket '{supermarket_id}' has no active product URLs to crawl yet."
        )
    return engine.run_now(supermarket_id)
