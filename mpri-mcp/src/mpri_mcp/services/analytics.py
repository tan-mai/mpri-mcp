"""Price analytics: comparison, deep discounts, history, AI usage."""

from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
from zoneinfo import ZoneInfo

from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from mpri_mcp.db import Database
from mpri_mcp.db.models import CrawlRunItem, PriceRecord, Product, ProductUrl
from mpri_mcp.i18n import _
from mpri_mcp.services.errors import ValidationError
from mpri_mcp.services.products import resolve_product
from mpri_mcp.utils.text import FuzzyMatch
from mpri_mcp.utils.time import utcnow

STALE_AFTER = timedelta(days=3)


def discount_pct(original: int | None, price: int | None) -> float | None:
    if price is None or not original or original <= 0:
        return None
    if original <= price:
        return 0.0
    return round((original - price) / original * 100, 1)


@dataclass
class MarketPrice:
    supermarket_id: str
    supermarket_name: str
    url: str
    url_active: bool
    current_price: int | None = None
    original_price: int | None = None
    member_price: int | None = None
    discount_pct: float | None = None
    member_discount_pct: float | None = None
    in_stock: bool | None = None
    crawled_at: datetime | None = None
    is_stale: bool = False
    last_error: str | None = None


@dataclass
class Comparison:
    product: Product
    prices: list[MarketPrice]
    cheapest: MarketPrice | None
    alternatives: list[FuzzyMatch[int]]


@dataclass
class DiscountItem:
    product_id: int
    product_name: str
    category: str | None
    supermarket_id: str
    supermarket_name: str
    url: str
    current_price: int
    original_price: int | None
    member_price: int | None
    discount_pct: float
    member_discount_pct: float | None
    crawled_at: datetime


@dataclass
class HistoryPoint:
    day: date
    current_price: int
    original_price: int | None
    member_price: int | None


@dataclass
class MarketHistory:
    supermarket_id: str
    supermarket_name: str
    points: list[HistoryPoint] = field(default_factory=list)

    @property
    def prices(self) -> list[int]:
        return [p.current_price for p in self.points]

    @property
    def min_point(self) -> HistoryPoint | None:
        return min(self.points, key=lambda p: (p.current_price, p.day)) if self.points else None

    @property
    def max_price(self) -> int | None:
        return max(self.prices) if self.points else None

    @property
    def avg_price(self) -> int | None:
        return round(sum(self.prices) / len(self.points)) if self.points else None

    @property
    def latest(self) -> HistoryPoint | None:
        return self.points[-1] if self.points else None


@dataclass
class History:
    product: Product
    days: int
    series: list[MarketHistory]
    alternatives: list[FuzzyMatch[int]] = field(default_factory=list)


@dataclass
class UsageRow:
    day: date
    provider: str
    model: str
    calls: int
    input_tokens: int
    output_tokens: int


async def _latest_records(
    s: AsyncSession, url_ids: list[int] | None = None, since: datetime | None = None
) -> dict[int, PriceRecord]:
    sub = select(PriceRecord.product_url_id, func.max(PriceRecord.crawled_at).label("mx"))
    if url_ids is not None:
        sub = sub.where(PriceRecord.product_url_id.in_(url_ids))
    if since is not None:
        sub = sub.where(PriceRecord.crawled_at >= since)
    sub = sub.group_by(PriceRecord.product_url_id).subquery()
    rows = await s.scalars(
        select(PriceRecord).join(
            sub,
            (PriceRecord.product_url_id == sub.c.product_url_id)
            & (PriceRecord.crawled_at == sub.c.mx),
        )
    )
    return {r.product_url_id: r for r in rows}


def _market_price(u: ProductUrl, rec: PriceRecord | None, now: datetime) -> MarketPrice:
    mp = MarketPrice(
        supermarket_id=u.supermarket_id,
        supermarket_name=u.supermarket.name,
        url=u.url,
        url_active=u.is_active,
        last_error=u.last_error if u.consecutive_failures else None,
    )
    if rec is not None:
        mp.current_price = rec.current_price
        mp.original_price = rec.original_price
        mp.member_price = rec.member_price
        mp.discount_pct = discount_pct(rec.original_price, rec.current_price)
        mp.member_discount_pct = (
            discount_pct(rec.original_price or rec.current_price, rec.member_price)
            if rec.member_price
            else None
        )
        mp.in_stock = rec.in_stock
        mp.crawled_at = rec.crawled_at
        mp.is_stale = now - rec.crawled_at > STALE_AFTER
    return mp


async def latest_prices(db: Database, product: Product) -> list[MarketPrice]:
    now = utcnow()
    urls = list(product.urls)
    async with db.session() as s:
        latest = await _latest_records(s, [u.id for u in urls])
    prices = [_market_price(u, latest.get(u.id), now) for u in urls]
    prices.sort(key=lambda p: (p.current_price is None, p.current_price or 0, p.supermarket_name))
    return prices


async def price_comparison(db: Database, product_name: str) -> Comparison:
    product, alternatives = await resolve_product(db, product_name)
    prices = await latest_prices(db, product)
    priced = [p for p in prices if p.current_price is not None]
    return Comparison(
        product=product,
        prices=prices,
        cheapest=priced[0] if priced else None,
        alternatives=alternatives,
    )


async def deep_discounts(
    db: Database, min_discount_percentage: float, days: int = 7, limit: int = 50
) -> list[DiscountItem]:
    if not 0 < min_discount_percentage < 100:
        raise ValidationError(_("min_discount_percentage must be between 1 and 99."))
    if not 1 <= days <= 365:
        raise ValidationError(_("days must be between 1 and {max}.").format(max=365))
    since = utcnow() - timedelta(days=days)
    async with db.session() as s:
        latest = await _latest_records(s, since=since)
        if not latest:
            return []
        urls = (
            await s.scalars(
                select(ProductUrl)
                .where(ProductUrl.id.in_(latest.keys()), ProductUrl.is_active)
                .options(selectinload(ProductUrl.product), selectinload(ProductUrl.supermarket))
            )
        ).all()
    items = []
    for u in urls:
        rec = latest[u.id]
        pct = discount_pct(rec.original_price, rec.current_price)
        if pct is None or pct < min_discount_percentage:
            continue
        items.append(
            DiscountItem(
                product_id=u.product_id,
                product_name=u.product.name,
                category=u.product.category,
                supermarket_id=u.supermarket_id,
                supermarket_name=u.supermarket.name,
                url=u.url,
                current_price=rec.current_price,
                original_price=rec.original_price,
                member_price=rec.member_price,
                discount_pct=pct,
                member_discount_pct=(
                    discount_pct(rec.original_price, rec.member_price) if rec.member_price else None
                ),
                crawled_at=rec.crawled_at,
            )
        )
    items.sort(key=lambda i: (-i.discount_pct, i.product_name))
    return items[: max(1, min(limit, 500))]


async def product_history(
    db: Database,
    product: Product,
    days: int = 30,
    supermarket_id: str | None = None,
    tz: str = "Asia/Ho_Chi_Minh",
) -> History:
    if not 1 <= days <= 730:
        raise ValidationError(_("days must be between 1 and {max}.").format(max=730))
    zone = ZoneInfo(tz)
    since = utcnow() - timedelta(days=days)
    urls = [u for u in product.urls if supermarket_id in (None, u.supermarket_id)]
    async with db.session() as s:
        records = (
            await s.scalars(
                select(PriceRecord)
                .where(
                    PriceRecord.product_url_id.in_([u.id for u in urls]),
                    PriceRecord.crawled_at >= since,
                )
                .order_by(PriceRecord.crawled_at)
            )
        ).all()
    by_url: dict[int, dict[date, PriceRecord]] = defaultdict(dict)
    for rec in records:  # ascending, so the last record of each local day wins
        by_url[rec.product_url_id][rec.crawled_at.astimezone(zone).date()] = rec
    series = []
    for u in sorted(urls, key=lambda u: u.supermarket.name):
        days_map = by_url.get(u.id, {})
        series.append(
            MarketHistory(
                supermarket_id=u.supermarket_id,
                supermarket_name=u.supermarket.name,
                points=[
                    HistoryPoint(d, r.current_price, r.original_price, r.member_price)
                    for d, r in sorted(days_map.items())
                ],
            )
        )
    return History(product=product, days=days, series=series)


async def price_history(
    db: Database,
    product_name: str,
    days: int = 30,
    supermarket_id: str | None = None,
    tz: str = "Asia/Ho_Chi_Minh",
) -> History:
    product, alternatives = await resolve_product(db, product_name)
    history = await product_history(db, product, days, supermarket_id, tz)
    history.alternatives = alternatives
    return history


async def ai_usage(db: Database, days: int = 7, tz: str = "Asia/Ho_Chi_Minh") -> list[UsageRow]:
    zone = ZoneInfo(tz)
    since = utcnow() - timedelta(days=days)
    async with db.session() as s:
        rows = (
            await s.execute(
                select(
                    CrawlRunItem.created_at,
                    CrawlRunItem.provider,
                    CrawlRunItem.model,
                    CrawlRunItem.input_tokens,
                    CrawlRunItem.output_tokens,
                ).where(CrawlRunItem.created_at >= since, CrawlRunItem.provider.is_not(None))
            )
        ).all()
    agg: dict[tuple[date, str, str], list[int]] = defaultdict(lambda: [0, 0, 0])
    for created_at, provider, model, tin, tout in rows:
        key = (created_at.astimezone(zone).date(), provider, model or "?")
        agg[key][0] += 1
        agg[key][1] += tin or 0
        agg[key][2] += tout or 0
    return [
        UsageRow(day=d, provider=p, model=m, calls=v[0], input_tokens=v[1], output_tokens=v[2])
        for (d, p, m), v in sorted(agg.items(), reverse=True)
    ]
