"""Message content for notifications (Vietnamese): price alerts and the summary sent after
each finished agent crawl. Messages are lists of lines made of segments, so each channel can
render bold text and links its own way."""

# TM - #{ticket number} - MPriMCP Price Alerts & Notifications

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from zoneinfo import ZoneInfo

from sqlalchemy import select
from sqlalchemy.orm import selectinload

from mpri_mcp.db import Database
from mpri_mcp.db.models import CrawlTask, Product, ProductUrl
from mpri_mcp.services import alerts
from mpri_mcp.services.analytics import cheapest_by_product, discount_pct


@dataclass
class Seg:
    text: str
    bold: bool = False
    url: str | None = None


Line = list[Seg]


def vnd(value: int | None) -> str:
    return "—" if value is None else f"{value:,}".replace(",", ".") + " ₫"


def plain(lines: list[Line]) -> str:
    return "\n".join("".join(s.text for s in line) for line in lines)


def _discount(original: int | None, price: int) -> list[Seg]:
    pct = discount_pct(original, price)
    if not pct:
        return []
    return [Seg(f" (gốc {vnd(original)}, −{pct:g}%)")]


def alert_lines(hit: alerts.AlertHit) -> list[Line]:
    lines: list[Line] = [
        [
            Seg("🔔 Giá dưới mốc: "),
            Seg(hit.product_name, bold=True),
            Seg(f" (mốc {vnd(hit.target_price)})"),
        ]
    ]
    for p in hit.prices:
        line = [Seg("• "), Seg(vnd(p.price), bold=True), Seg(f" tại {p.supermarket_name}")]
        line += _discount(p.original_price, p.price)
        if p.member_price:
            line.append(Seg(f" · thành viên {vnd(p.member_price)}"))
        line += [Seg(" · "), Seg("Xem sản phẩm", url=p.url)]
        if p.needs_review:
            line.append(Seg(" ⚠ chưa xác minh"))
        lines.append(line)
    if any(p.needs_review for p in hit.prices):
        lines.append([Seg("⚠ Giá lệch hơn 50% so với lần trước: hãy kiểm tra lại trước khi mua.")])
    return lines


def test_lines() -> list[Line]:
    return [[Seg("✅ MPRI: kênh thông báo hoạt động.", bold=True)]]


async def summary_lines(db: Database, since: datetime | None, now: datetime, tz: str) -> list[Line]:
    """Where each tracked product is cheapest right now. Products with a price alert are
    shown once, in the alert section, with whether their target is met."""
    zone = ZoneInfo(tz)
    async with db.session() as s:
        active = select(ProductUrl.product_id).where(ProductUrl.is_active)
        products = (
            await s.scalars(select(Product).where(Product.id.in_(active)).order_by(Product.name))
        ).all()
        failed_stmt = (
            select(CrawlTask)
            .where(CrawlTask.status == "failed")
            .options(
                selectinload(CrawlTask.product_url).selectinload(ProductUrl.product),
                selectinload(CrawlTask.product_url).selectinload(ProductUrl.supermarket),
            )
            .order_by(CrawlTask.completed_at)
        )
        if since is not None:
            failed_stmt = failed_stmt.where(CrawlTask.completed_at >= since)
        failed = (await s.scalars(failed_stmt)).all()
    cheapest = await cheapest_by_product(db)
    alert_views = {v.product.id: v for v in await alerts.list_alerts(db)}

    lines: list[Line] = [
        [Seg(f"🛒 Giá rẻ nhất — lượt quét {now.astimezone(zone):%d/%m %H:%M}", bold=True)]
    ]
    plain_products = [p for p in products if p.id not in alert_views]
    for product in plain_products:
        mp = cheapest.get(product.id)
        if mp is None or mp.current_price is None:
            lines.append([Seg(f"• {product.name} — chưa có giá")])
            continue
        line = [
            Seg("• "),
            Seg(product.name, url=mp.url),
            Seg(" — "),
            Seg(vnd(mp.current_price), bold=True),
            Seg(f" tại {mp.supermarket_name}"),
        ]
        line += _discount(mp.original_price, mp.current_price)
        if mp.needs_review:
            line.append(Seg(" ⚠ chưa xác minh"))
        if since is not None and mp.crawled_at is not None and mp.crawled_at < since:
            line.append(Seg(f" · giá ngày {mp.crawled_at.astimezone(zone):%d/%m}"))
        lines.append(line)
    if not plain_products and not alert_views:
        lines.append([Seg("Chưa theo dõi sản phẩm nào.")])

    if alert_views:
        lines.append([Seg("🎯 Mốc giá", bold=True)])
        for v in sorted(alert_views.values(), key=lambda v: v.product.name):
            head = [Seg("• "), Seg(v.product.name), Seg(f" < {vnd(v.alert.target_price)}: ")]
            if v.best is None or v.best.current_price is None:
                lines.append([*head, Seg("chưa có giá")])
            elif v.below:
                lines.append(
                    [
                        *head,
                        Seg("✅ đạt — "),
                        Seg(vnd(v.best.current_price), bold=True),
                        Seg(f" tại {v.best.supermarket_name}"),
                        Seg(" · "),
                        Seg("Xem", url=v.best.url),
                    ]
                )
            else:
                lines.append(
                    [
                        *head,
                        Seg(f"chưa đạt — rẻ nhất {vnd(v.best.current_price)}"),
                        Seg(f" tại {v.best.supermarket_name}"),
                    ]
                )

    if failed:
        lines.append([Seg("⛔ Không đọc được lượt này", bold=True)])
        for t in failed:
            u = t.product_url
            lines.append(
                [Seg(f"• {u.product.name} tại {u.supermarket.name}: {t.last_error or 'lỗi'}")]
            )
    return lines
