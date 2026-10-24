"""Supermarket management."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from sqlalchemy import case, func, select
from sqlalchemy.orm import selectinload

from mpri_mcp.crawler.config import CrawlConfig
from mpri_mcp.db import Database
from mpri_mcp.db.models import CrawlRun, ProductUrl, Supermarket
from mpri_mcp.services.errors import ConflictError, NotFoundError, ValidationError
from mpri_mcp.utils.text import normalize_base_url, normalize_name, slugify

EDITABLE_FIELDS = {"name", "base_url", "crawl_config", "ai_provider", "ai_model", "timezone"}


@dataclass
class SupermarketSummary:
    supermarket: Supermarket
    url_count: int
    active_url_count: int
    failing_url_count: int
    last_run: CrawlRun | None


async def get_supermarket(db: Database, supermarket_id: str) -> Supermarket:
    async with db.session() as s:
        market = await s.get(Supermarket, supermarket_id)
    if market is None:
        raise NotFoundError(
            f"Supermarket '{supermarket_id}' not found. Use get_supermarkets to list valid IDs."
        )
    return market


async def list_supermarkets(db: Database) -> list[SupermarketSummary]:
    async with db.session() as s:
        markets = (await s.scalars(select(Supermarket).order_by(Supermarket.name))).all()
        counts = {
            row.supermarket_id: row
            for row in await s.execute(
                select(
                    ProductUrl.supermarket_id,
                    func.count().label("total"),
                    func.sum(case((ProductUrl.is_active, 1), else_=0)).label("active"),
                    func.sum(
                        case(
                            (ProductUrl.is_active & (ProductUrl.consecutive_failures > 0), 1),
                            else_=0,
                        )
                    ).label("failing"),
                ).group_by(ProductUrl.supermarket_id)
            )
        }
        latest_ids = select(func.max(CrawlRun.id)).group_by(CrawlRun.supermarket_id)
        last_runs = {
            run.supermarket_id: run
            for run in (await s.scalars(select(CrawlRun).where(CrawlRun.id.in_(latest_ids)))).all()
        }
    result = []
    for m in markets:
        c = counts.get(m.id)
        result.append(
            SupermarketSummary(
                supermarket=m,
                url_count=c.total if c else 0,
                active_url_count=int(c.active or 0) if c else 0,
                failing_url_count=int(c.failing or 0) if c else 0,
                last_run=last_runs.get(m.id),
            )
        )
    return result


async def add_supermarket(db: Database, name: str, base_url: str) -> Supermarket:
    name = " ".join(name.split())
    if not name:
        raise ValidationError("Supermarket name is required.")
    if len(name) > 200:
        raise ValidationError("Supermarket name is too long (max 200 characters).")
    try:
        base = normalize_base_url(base_url)
    except ValueError as e:
        raise ValidationError(str(e)) from e

    async with db.session() as s:
        existing = (await s.scalars(select(Supermarket))).all()
        wanted = normalize_name(name)
        for m in existing:
            if normalize_name(m.name) == wanted:
                raise ConflictError(f"Supermarket '{m.name}' already exists with ID '{m.id}'.")
        taken = {m.id for m in existing}
        base_slug = slugify(name)[:60] or "market"
        slug, n = base_slug, 2
        while slug in taken:
            slug, n = f"{base_slug}-{n}", n + 1
        market = Supermarket(id=slug, name=name, base_url=base, crawl_config={})
        s.add(market)
    return market


async def update_supermarket(db: Database, supermarket_id: str, **changes: Any) -> Supermarket:
    unknown = set(changes) - EDITABLE_FIELDS
    if unknown:
        raise ValidationError(f"Cannot edit fields: {', '.join(sorted(unknown))}")
    async with db.session() as s:
        market = await s.get(Supermarket, supermarket_id)
        if market is None:
            raise NotFoundError(f"Supermarket '{supermarket_id}' not found.")
        if "name" in changes:
            name = " ".join(str(changes["name"]).split())
            if not name:
                raise ValidationError("Supermarket name is required.")
            others = (
                await s.scalars(select(Supermarket).where(Supermarket.id != supermarket_id))
            ).all()
            if any(normalize_name(o.name) == normalize_name(name) for o in others):
                raise ConflictError(f"Another supermarket is already named '{name}'.")
            market.name = name
        if "base_url" in changes:
            try:
                market.base_url = normalize_base_url(str(changes["base_url"]))
            except ValueError as e:
                raise ValidationError(str(e)) from e
        if "crawl_config" in changes:
            try:
                market.crawl_config = CrawlConfig.model_validate(
                    changes["crawl_config"] or {}
                ).model_dump(exclude_defaults=True)
            except ValueError as e:
                raise ValidationError(f"Invalid crawl_config: {e}") from e
        for key in ("ai_provider", "ai_model"):
            if key in changes:
                setattr(market, key, (changes[key] or None))
        if "timezone" in changes:
            try:
                ZoneInfo(changes["timezone"])
            except (ZoneInfoNotFoundError, ValueError) as e:
                raise ValidationError(f"Unknown timezone '{changes['timezone']}'.") from e
            market.timezone = changes["timezone"]
    return market


async def set_schedule_fields(
    db: Database, supermarket_id: str, *, cron_expression: str | None, cron_enabled: bool
) -> Supermarket:
    async with db.session() as s:
        market = await s.get(Supermarket, supermarket_id)
        if market is None:
            raise NotFoundError(f"Supermarket '{supermarket_id}' not found.")
        market.cron_expression = cron_expression
        market.cron_enabled = cron_enabled
    return market


async def delete_supermarket(db: Database, supermarket_id: str) -> None:
    async with db.session() as s:
        market = await s.get(Supermarket, supermarket_id)
        if market is None:
            raise NotFoundError(f"Supermarket '{supermarket_id}' not found.")
        await s.delete(market)


async def market_urls(db: Database, supermarket_id: str) -> list[ProductUrl]:
    async with db.session() as s:
        return list(
            (
                await s.scalars(
                    select(ProductUrl)
                    .where(ProductUrl.supermarket_id == supermarket_id)
                    .options(selectinload(ProductUrl.product))
                )
            ).all()
        )
