"""Supermarket management."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any
from urllib.parse import quote, urlparse
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from sqlalchemy import case, func, select
from sqlalchemy.orm import selectinload

from mpri_mcp.db import Database
from mpri_mcp.db.models import CrawlRun, CrawlTask, PriceRecord, ProductUrl, Supermarket
from mpri_mcp.services.errors import ConflictError, NotFoundError, ValidationError
from mpri_mcp.utils.text import normalize_base_url, normalize_name, slugify

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
EDITABLE_FIELDS = {"name", "base_url", "timezone", "search_url_template"}
SEARCH_PLACEHOLDER = "{q}"
SEARCH_NOTE = "open base_url and use the site's search box"
OPEN_TASK_STATUSES = ("pending", "claimed")


@dataclass
class SupermarketSummary:
    supermarket: Supermarket
    url_count: int
    active_url_count: int
    failing_url_count: int
    last_run: CrawlRun | None
    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    pending_tasks: int = 0
    needs_review_count: int = 0


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
@dataclass
class SearchUrl:
    supermarket: Supermarket
    url: str | None


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def clean_search_url_template(value: str | None) -> str | None:
    """'' or None clears the template. Otherwise: an http(s) URL with exactly one {q}."""
    template = (value or "").strip()
    if not template:
        return None
    if len(template) > 500:
        raise ValidationError("search_url_template is too long (max 500 characters).")
    if template.count(SEARCH_PLACEHOLDER) != 1:
        raise ValidationError(
            "search_url_template must contain the placeholder {q} exactly once, e.g. "
            "'https://cooponline.vn/search?router=productListing&query={q}'."
        )
    parsed = urlparse(template.replace(SEARCH_PLACEHOLDER, "q"))
    if parsed.scheme not in ("http", "https") or not parsed.netloc or "." not in parsed.netloc:
        raise ValidationError("search_url_template must be a full http(s) URL.")
    return template


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def build_search_url(template: str | None, keyword: str) -> str | None:
    if not template:
        return None
    return template.replace(SEARCH_PLACEHOLDER, quote(keyword, safe=""))


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
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        pending: dict[str, int] = dict(
            (
                await s.execute(
                    select(CrawlTask.supermarket_id, func.count())
                    .where(CrawlTask.status.in_(OPEN_TASK_STATUSES))
                    .group_by(CrawlTask.supermarket_id)
                )
            ).all()
        )
        flagged: dict[str, int] = dict(
            (
                await s.execute(
                    select(ProductUrl.supermarket_id, func.count())
                    .join(PriceRecord, PriceRecord.product_url_id == ProductUrl.id)
                    .where(PriceRecord.needs_review)
                    .group_by(ProductUrl.supermarket_id)
                )
            ).all()
        )
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
                pending_tasks=pending.get(m.id, 0),
                needs_review_count=flagged.get(m.id, 0),
            )
        )
    return result


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
async def search_urls(
    db: Database, keyword: str, supermarket_ids: list[str] | None = None
) -> list[SearchUrl]:
    """Search-results URLs for a keyword. Builds strings only; nothing is fetched."""
    keyword = " ".join((keyword or "").split())
    if not keyword:
        raise ValidationError("keyword is required.")
    if len(keyword) > 200:
        raise ValidationError("keyword is too long (max 200 characters).")
    async with db.session() as s:
        markets = list((await s.scalars(select(Supermarket).order_by(Supermarket.name))).all())
    if supermarket_ids:
        wanted = list(dict.fromkeys(supermarket_ids))
        known = {m.id: m for m in markets}
        unknown = [i for i in wanted if i not in known]
        if unknown:
            raise NotFoundError(
                f"Unknown supermarket ID(s): {', '.join(unknown)}. "
                "Use get_supermarkets to list valid IDs."
            )
        markets = [known[i] for i in wanted]
    return [SearchUrl(m, build_search_url(m.search_url_template, keyword)) for m in markets]


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
async def add_supermarket(
    db: Database, name: str, base_url: str, search_url_template: str | None = None
) -> Supermarket:
    name = " ".join(name.split())
    if not name:
        raise ValidationError("Supermarket name is required.")
    if len(name) > 200:
        raise ValidationError("Supermarket name is too long (max 200 characters).")
    try:
        base = normalize_base_url(base_url)
    except ValueError as e:
        raise ValidationError(str(e)) from e
    template = clean_search_url_template(search_url_template)

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
        market = Supermarket(id=slug, name=name, base_url=base, search_url_template=template)
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
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        if "search_url_template" in changes:
            market.search_url_template = clean_search_url_template(changes["search_url_template"])
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
