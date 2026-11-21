"""Prices flagged needs_review (more than 50% away from the previous price): list, accept,
reject. Used by the dashboard."""

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search

from __future__ import annotations

import logging
from dataclasses import dataclass

from sqlalchemy import func, select
from sqlalchemy.orm import selectinload

from mpri_mcp.db import Database
from mpri_mcp.db.models import PriceRecord, ProductUrl
from mpri_mcp.services.errors import NotFoundError

log = logging.getLogger(__name__)


@dataclass
class ReviewItem:
    record: PriceRecord
    product_id: int
    product_name: str
    supermarket_name: str
    url: str
    previous_price: int | None


async def count_needs_review(db: Database) -> int:
    async with db.session() as s:
        return (
            await s.scalar(
                select(func.count()).select_from(PriceRecord).where(PriceRecord.needs_review)
            )
            or 0
        )


async def list_needs_review(db: Database, limit: int = 50) -> list[ReviewItem]:
    async with db.session() as s:
        records = (
            await s.scalars(
                select(PriceRecord)
                .where(PriceRecord.needs_review)
                .order_by(PriceRecord.crawled_at.desc())
                .limit(limit)
                .options(
                    selectinload(PriceRecord.product_url).selectinload(ProductUrl.product),
                    selectinload(PriceRecord.product_url).selectinload(ProductUrl.supermarket),
                )
            )
        ).all()
        items = []
        for rec in records:
            previous = await s.scalar(
                select(PriceRecord.current_price)
                .where(
                    PriceRecord.product_url_id == rec.product_url_id,
                    PriceRecord.crawled_at < rec.crawled_at,
                )
                .order_by(PriceRecord.crawled_at.desc())
                .limit(1)
            )
            u = rec.product_url
            items.append(
                ReviewItem(
                    record=rec,
                    product_id=u.product_id,
                    product_name=u.product.name,
                    supermarket_name=u.supermarket.name,
                    url=u.url,
                    previous_price=previous,
                )
            )
    return items


async def accept_price(db: Database, record_id: int) -> None:
    """The price is right: clear the flag."""
    async with db.session() as s:
        rec = await s.get(PriceRecord, record_id)
        if rec is None:
            raise NotFoundError(f"Price #{record_id} not found.")
        rec.needs_review = False
    log.info("Price #%s accepted after review", record_id)


async def reject_price(db: Database, record_id: int) -> None:
    """The price is wrong: delete the record."""
    async with db.session() as s:
        rec = await s.get(PriceRecord, record_id)
        if rec is None:
            raise NotFoundError(f"Price #{record_id} not found.")
        await s.delete(rec)
    log.info("Price #%s rejected after review and deleted", record_id)
