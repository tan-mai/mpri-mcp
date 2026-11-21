"""Price alerts: tell the user when a tracked product's price drops below their target."""

# TM - #{ticket number} - MPriMCP Price Alerts & Notifications

from __future__ import annotations

import logging
from collections import defaultdict
from dataclasses import dataclass, field

from sqlalchemy import select
from sqlalchemy.orm import selectinload

from mpri_mcp.db import Database
from mpri_mcp.db.models import PriceAlert, Product, ProductUrl
from mpri_mcp.services.analytics import MarketPrice, latest_prices
from mpri_mcp.services.errors import NotFoundError, ValidationError
from mpri_mcp.services.products import get_product, resolve_product
from mpri_mcp.utils.text import fuzzy_rank

log = logging.getLogger(__name__)

MAX_TARGET = 1_000_000_000


@dataclass
class SavedPrice:
    """A price just saved by submit_price_results."""

    product_id: int
    product_name: str
    supermarket_id: str
    supermarket_name: str
    url: str
    price: int
    original_price: int | None = None
    member_price: int | None = None
    needs_review: bool = False


@dataclass
class AlertHit:
    product_id: int
    product_name: str
    target_price: int
    prices: list[SavedPrice] = field(default_factory=list)


@dataclass
class AlertView:
    alert: PriceAlert
    product: Product
    best: MarketPrice | None  # cheapest latest price at the supermarkets the alert covers

    @property
    def below(self) -> bool:
        return bool(self.best and self.best.current_price is not None) and (
            self.best.current_price < self.alert.target_price  # type: ignore[union-attr,operator]
        )


def _covers(alert: PriceAlert, supermarket_id: str) -> bool:
    return not alert.supermarket_ids or supermarket_id in alert.supermarket_ids


async def _view(db: Database, alert: PriceAlert, product: Product) -> AlertView:
    prices = [
        p
        for p in await latest_prices(db, product)
        if p.url_active and p.current_price is not None and _covers(alert, p.supermarket_id)
    ]
    return AlertView(alert, product, prices[0] if prices else None)  # cheapest first


async def _load(db: Database, *where: object) -> list[PriceAlert]:
    async with db.session() as s:
        return list(
            (
                await s.scalars(
                    select(PriceAlert)
                    .where(*where)
                    .options(
                        selectinload(PriceAlert.product)
                        .selectinload(Product.urls)
                        .selectinload(ProductUrl.supermarket)
                    )
                )
            ).all()
        )


async def set_alert(
    db: Database, product_name: str, target_price: int, supermarket_ids: list[str] | None = None
) -> tuple[AlertView, bool]:
    """Create or replace the alert of the best-matching product. Returns (view, created)."""
    product, _ = await resolve_product(db, product_name)
    return await _save(db, product, target_price, supermarket_ids)


async def set_alert_for_product(
    db: Database, product_id: int, target_price: int, supermarket_ids: list[str] | None = None
) -> tuple[AlertView, bool]:
    return await _save(db, await get_product(db, product_id), target_price, supermarket_ids)


async def _save(
    db: Database, product: Product, target_price: int, supermarket_ids: list[str] | None
) -> tuple[AlertView, bool]:
    if not 0 < target_price <= MAX_TARGET:
        raise ValidationError("target_price must be a positive price in VND, e.g. 70000.")
    ids = list(dict.fromkeys(supermarket_ids or [])) or None
    if ids:
        tracked = {u.supermarket_id for u in product.urls}
        missing = [i for i in ids if i not in tracked]
        if missing:
            raise ValidationError(
                f"'{product.name}' is not tracked at: {', '.join(missing)}. "
                f"It is tracked at: {', '.join(sorted(tracked)) or 'no supermarket yet'}."
            )
    async with db.session() as s:
        alert = await s.scalar(select(PriceAlert).where(PriceAlert.product_id == product.id))
        created = alert is None
        if alert is None:
            alert = PriceAlert(product_id=product.id, target_price=target_price)
            s.add(alert)
        alert.target_price = target_price
        alert.supermarket_ids = ids
        alert.notified = {}  # a new target: announce again from the next crawl
    log.info("Price alert for %s: below %d VND (%s)", product.name, target_price, ids or "all")
    return await _view(db, alert, product), created


async def list_alerts(db: Database) -> list[AlertView]:
    views = [await _view(db, a, a.product) for a in await _load(db)]
    return sorted(views, key=lambda v: v.product.name)


async def alert_for_product(db: Database, product_id: int) -> AlertView | None:
    found = await _load(db, PriceAlert.product_id == product_id)
    return await _view(db, found[0], found[0].product) if found else None


async def delete_alert(db: Database, product_name: str) -> str:
    """Delete the alert whose product best matches the name; returns the product name."""
    alerts = await _load(db)
    match = fuzzy_rank(product_name, [(a.id, a.product.name) for a in alerts], limit=1)
    if not match:
        raise NotFoundError(f"No price alert matches '{product_name}'. Use get_price_alerts.")
    await delete_alert_by_id(db, match[0].key)
    return match[0].name


async def delete_alert_by_id(db: Database, alert_id: int) -> None:
    async with db.session() as s:
        alert = await s.get(PriceAlert, alert_id)
        if alert is None:
            raise NotFoundError(f"Price alert #{alert_id} not found.")
        await s.delete(alert)
    log.info("Price alert #%s deleted", alert_id)


async def evaluate(db: Database, saved: list[SavedPrice]) -> list[AlertHit]:
    """Alerts set off by newly saved prices. A supermarket is announced once when its price
    drops below the target, again only if it drops further, and re-armed once the price is
    back at or above the target."""
    by_product: dict[int, list[SavedPrice]] = defaultdict(list)
    for p in saved:
        by_product[p.product_id].append(p)
    if not by_product:
        return []
    hits: list[AlertHit] = []
    async with db.session() as s:
        alerts = (
            await s.scalars(select(PriceAlert).where(PriceAlert.product_id.in_(by_product)))
        ).all()
        for alert in alerts:
            notified = dict(alert.notified or {})
            found: list[SavedPrice] = []
            for p in by_product[alert.product_id]:
                if not _covers(alert, p.supermarket_id):
                    continue
                if p.price < alert.target_price:
                    previous = notified.get(p.supermarket_id)
                    if previous is None or p.price < previous:
                        found.append(p)
                        notified[p.supermarket_id] = p.price
                else:
                    notified.pop(p.supermarket_id, None)
            alert.notified = notified
            if found:
                found.sort(key=lambda p: p.price)
                hits.append(
                    AlertHit(alert.product_id, found[0].product_name, alert.target_price, found)
                )
                log.info(
                    "Price alert hit: %s at %s below %d VND",
                    found[0].product_name,
                    ", ".join(f"{p.supermarket_id} {p.price}" for p in found),
                    alert.target_price,
                )
    return hits
