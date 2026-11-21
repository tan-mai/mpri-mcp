"""Crawl tasks for Claude: the server queues product pages, Claude reads them in the browser
and sends the prices back. The server itself never fetches a page."""

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search

from __future__ import annotations

import asyncio
import logging
import secrets
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from weakref import WeakKeyDictionary

from sqlalchemy import func, select, update
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from mpri_mcp.config import Settings
from mpri_mcp.db import Database
from mpri_mcp.db.models import (
    CrawlRun,
    CrawlRunItem,
    CrawlTask,
    PriceRecord,
    ProductUrl,
    Supermarket,
)
from mpri_mcp.services.analytics import latest_records
from mpri_mcp.services.errors import ValidationError
from mpri_mcp.services.supermarkets import OPEN_TASK_STATUSES, get_supermarket
from mpri_mcp.utils.time import utcnow

log = logging.getLogger(__name__)

RESULT_STATUSES = ("ok", "out_of_stock", "not_found", "error")
FINISHED_STATUSES = ("done", "failed", "expired")
MAX_LIMIT = 50
MAX_BATCH = 50
MAX_PLAUSIBLE_PRICE = 1_000_000_000  # 1 billion VND
REVIEW_CHANGE = 0.5  # a price more than 50% away from the last one is flagged needs_review
RAW_TEXT_MAX = 500
NOTE_MAX = 300

# One process, one uvicorn worker: a lock per database makes queueing, claiming and
# submitting atomic, so two parallel calls never get the same task.
_LOCKS: WeakKeyDictionary[Database, asyncio.Lock] = WeakKeyDictionary()


def _lock(db: Database) -> asyncio.Lock:
    lock = _LOCKS.get(db)
    if lock is None:
        lock = _LOCKS[db] = asyncio.Lock()
    return lock


@dataclass
class QueueResult:
    supermarket: Supermarket
    run_id: int | None
    queued: int
    already_open: int
    open_tasks: int


@dataclass
class ClaimedTask:
    task: CrawlTask
    product_name: str
    category: str | None
    supermarket_name: str
    last_price: int | None
    last_original_price: int | None
    last_checked_at: datetime | None


@dataclass
class ClaimResult:
    tasks: list[ClaimedTask]
    remaining_pending: int


@dataclass
class PriceResult:
    task_id: str
    status: str
    price: int | None = None
    original_price: int | None = None
    member_price: int | None = None
    raw_text: str | None = None
    note: str | None = None


@dataclass
class Flagged:
    task_id: str
    product_name: str
    supermarket_id: str
    price: int
    last_price: int
    change_pct: float


@dataclass
class Rejected:
    task_id: str
    reason: str


@dataclass
class SubmitResult:
    accepted: int = 0
    requeued: int = 0
    flagged: list[Flagged] = field(default_factory=list)
    rejected: list[Rejected] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    finished_runs: list[int] = field(default_factory=list)
    remaining_pending: int = 0


def _new_task_id() -> str:
    return secrets.token_hex(6)


async def _count_pending(s: AsyncSession, supermarket_id: str | None = None) -> int:
    stmt = select(func.count()).select_from(CrawlTask).where(CrawlTask.status == "pending")
    if supermarket_id:
        stmt = stmt.where(CrawlTask.supermarket_id == supermarket_id)
    return await s.scalar(stmt) or 0


async def _maintain(s: AsyncSession, settings: Settings, now: datetime) -> None:
    """Claimed tasks whose lease ran out go back to pending; tasks pending for too long
    expire (kept for reference) and their runs are closed."""
    released = await s.execute(
        update(CrawlTask)
        .where(CrawlTask.status == "claimed", CrawlTask.lease_expires_at < now)
        .values(status="pending", lease_expires_at=None)
        .execution_options(synchronize_session=False)
    )
    if released.rowcount:
        log.info("Released %d crawl task(s) whose lease expired", released.rowcount)

    cutoff = now - timedelta(hours=settings.agent_task_expire_hours)
    stale = (CrawlTask.status == "pending", CrawlTask.created_at < cutoff)
    run_ids = set((await s.scalars(select(CrawlTask.run_id).where(*stale).distinct())).all())
    expired = await s.execute(
        update(CrawlTask)
        .where(*stale)
        .values(
            status="expired",
            completed_at=now,
            last_error=f"Not crawled within {settings.agent_task_expire_hours:g} hours",
        )
        .execution_options(synchronize_session=False)
    )
    if expired.rowcount:
        log.warning("Expired %d crawl task(s) never crawled", expired.rowcount)
        for run_id in run_ids - {None}:
            await _finish_run_if_done(s, run_id, now)


async def _finish_run_if_done(s: AsyncSession, run_id: int, now: datetime) -> bool:
    """Once every task of a queued run has ended, record the outcome on the run."""
    counts = dict(
        (
            await s.execute(
                select(CrawlTask.status, func.count())
                .where(CrawlTask.run_id == run_id)
                .group_by(CrawlTask.status)
            )
        ).all()
    )
    if any(counts.get(st) for st in OPEN_TASK_STATUSES):
        return False
    run = await s.get(CrawlRun, run_id)
    if run is None or run.status != "queued":
        return False
    ok = counts.get("done", 0)
    failed = counts.get("failed", 0) + counts.get("expired", 0)
    run.urls_ok, run.urls_failed = ok, failed
    run.status = "success" if not failed else ("partial" if ok else "failed")
    run.trigger = "agent"
    run.finished_at = now
    log.info(
        "Agent crawl run #%s of %s finished: %s (%d ok, %d failed)",
        run.id,
        run.supermarket_id,
        run.status,
        ok,
        failed,
    )
    return True


async def maintain(db: Database, settings: Settings) -> None:
    async with _lock(db), db.session() as s:
        await _maintain(s, settings, utcnow())


# ---------------------------------------------------------------------- queue


async def queue_market(
    db: Database, settings: Settings, supermarket_id: str, trigger: str
) -> QueueResult:
    """Queue a pending task for every active URL of the supermarket (cron or manual).
    URLs that already have a pending or claimed task are skipped."""
    market = await get_supermarket(db, supermarket_id)
    now = utcnow()
    async with _lock(db), db.session() as s:
        await _maintain(s, settings, now)
        urls = (
            await s.scalars(
                select(ProductUrl)
                .where(ProductUrl.supermarket_id == supermarket_id, ProductUrl.is_active)
                .order_by(ProductUrl.id)
            )
        ).all()
        open_ids = set(
            (
                await s.scalars(
                    select(CrawlTask.product_url_id).where(
                        CrawlTask.supermarket_id == supermarket_id,
                        CrawlTask.status.in_(OPEN_TASK_STATUSES),
                    )
                )
            ).all()
        )
        new = [u for u in urls if u.id not in open_ids]
        run_id = None
        if new:
            run = CrawlRun(
                supermarket_id=supermarket_id,
                trigger=trigger,
                status="queued",
                started_at=now,
                urls_total=len(new),
            )
            s.add(run)
            await s.flush()
            run_id = run.id
            for u in new:
                s.add(
                    CrawlTask(
                        id=_new_task_id(),
                        product_url_id=u.id,
                        product_id=u.product_id,
                        supermarket_id=supermarket_id,
                        url=u.url,
                        status="pending",
                        attempts=0,
                        created_at=now,
                        run_id=run_id,
                    )
                )
    already_open = len(urls) - len(new)
    log.info(
        "Queued %d crawl task(s) for %s (%s, run #%s); %d already open",
        len(new),
        supermarket_id,
        trigger,
        run_id,
        already_open,
    )
    return QueueResult(market, run_id, len(new), already_open, len(open_ids) + len(new))


# ---------------------------------------------------------------------- claim


async def claim_tasks(
    db: Database, settings: Settings, supermarket_id: str | None = None, limit: int = 20
) -> ClaimResult:
    """Reserve the oldest pending tasks for the caller (lease)."""
    if not 1 <= limit <= MAX_LIMIT:
        raise ValidationError(f"limit must be between 1 and {MAX_LIMIT}.")
    if supermarket_id:
        await get_supermarket(db, supermarket_id)
    now = utcnow()
    lease_until = now + timedelta(minutes=settings.agent_task_lease_minutes)
    async with _lock(db), db.session() as s:
        await _maintain(s, settings, now)
        stmt = select(CrawlTask.id).where(CrawlTask.status == "pending")
        if supermarket_id:
            stmt = stmt.where(CrawlTask.supermarket_id == supermarket_id)
        ids = list(
            (
                await s.scalars(
                    stmt.order_by(CrawlTask.created_at, CrawlTask.product_url_id).limit(limit)
                )
            ).all()
        )
        tasks: list[CrawlTask] = []
        if ids:
            await s.execute(
                update(CrawlTask)
                .where(CrawlTask.id.in_(ids), CrawlTask.status == "pending")
                .values(
                    status="claimed",
                    claimed_at=now,
                    lease_expires_at=lease_until,
                    attempts=CrawlTask.attempts + 1,
                )
                .execution_options(synchronize_session=False)
            )
            loaded = {
                t.id: t
                for t in (
                    await s.scalars(
                        select(CrawlTask)
                        .where(CrawlTask.id.in_(ids), CrawlTask.status == "claimed")
                        .options(
                            selectinload(CrawlTask.product_url).selectinload(ProductUrl.product),
                            selectinload(CrawlTask.product_url).selectinload(
                                ProductUrl.supermarket
                            ),
                        )
                        .execution_options(populate_existing=True)
                    )
                ).all()
            }
            tasks = [loaded[i] for i in ids if i in loaded]
        latest = await latest_records(s, [t.product_url_id for t in tasks])
        remaining = await _count_pending(s, supermarket_id)
    claimed = []
    for t in tasks:
        rec = latest.get(t.product_url_id)
        claimed.append(
            ClaimedTask(
                task=t,
                product_name=t.product_url.product.name,
                category=t.product_url.product.category,
                supermarket_name=t.product_url.supermarket.name,
                last_price=rec.current_price if rec else None,
                last_original_price=rec.original_price if rec else None,
                last_checked_at=rec.crawled_at if rec else None,
            )
        )
    log.info(
        "Claimed %d crawl task(s)%s until %s; %d still pending",
        len(claimed),
        f" of {supermarket_id}" if supermarket_id else "",
        lease_until.isoformat(timespec="minutes"),
        remaining,
    )
    return ClaimResult(tasks=claimed, remaining_pending=remaining)


# ---------------------------------------------------------------------- submit


def _problem(r: PriceResult, task: CrawlTask | None, seen: set[str]) -> str | None:
    """Why this result cannot be accepted, or None."""
    if r.task_id in seen:
        return "Duplicate task_id in this batch."
    if task is None:
        return "Unknown task_id."
    if task.status != "claimed" and not (task.status == "pending" and task.claimed_at):
        if task.status == "pending":
            return "Task is pending; claim it with get_pending_crawl_tasks first."
        return f"Task is already {task.status}."
    if r.status not in RESULT_STATUSES:
        return f"status must be one of: {', '.join(RESULT_STATUSES)}."
    if r.raw_text and len(r.raw_text) > RAW_TEXT_MAX:
        return f"raw_text is longer than {RAW_TEXT_MAX} characters."
    if r.note and len(r.note) > NOTE_MAX:
        return f"note is longer than {NOTE_MAX} characters."
    if r.status != "ok":
        return None
    if r.price is None or r.price <= 0:
        return "status 'ok' needs a price greater than 0."
    if r.price > MAX_PLAUSIBLE_PRICE:
        return f"price {r.price} is implausibly high."
    if r.original_price is not None and r.original_price < r.price:
        return f"original_price ({r.original_price}) is lower than price ({r.price})."
    if r.member_price is not None and r.member_price <= 0:
        return "member_price must be greater than 0."
    return None


def _mark_success(row: ProductUrl, now: datetime, in_stock: bool) -> None:
    row.last_crawled_at = now
    row.last_success_at = now
    row.last_error = None
    row.consecutive_failures = 0
    row.in_stock = in_stock


def _mark_failure(row: ProductUrl, now: datetime, reason: str) -> None:
    row.last_crawled_at = now
    row.last_error = reason
    row.consecutive_failures += 1


def _end_task(
    s: AsyncSession, task: CrawlTask, status: str, now: datetime, item: str, note: str | None
) -> None:
    task.status = status
    task.completed_at = now
    task.lease_expires_at = None
    task.last_error = note if status == "failed" else None
    if task.run_id is not None:
        s.add(
            CrawlRunItem(
                run_id=task.run_id,
                product_url_id=task.product_url_id,
                url=task.url,
                status=item,
                error=note,
            )
        )


async def submit_results(
    db: Database, settings: Settings, results: list[PriceResult]
) -> SubmitResult:
    if not results:
        raise ValidationError("results is empty.")
    if len(results) > MAX_BATCH:
        raise ValidationError(f"Send at most {MAX_BATCH} results per call.")
    now = utcnow()
    out = SubmitResult()
    runs: set[int] = set()
    async with _lock(db), db.session() as s:
        await _maintain(s, settings, now)
        tasks = {
            t.id: t
            for t in (
                await s.scalars(
                    select(CrawlTask)
                    .where(CrawlTask.id.in_({r.task_id for r in results}))
                    .options(selectinload(CrawlTask.product_url).selectinload(ProductUrl.product))
                    .execution_options(populate_existing=True)
                )
            ).all()
        }
        latest = await latest_records(s, [t.product_url_id for t in tasks.values()])
        seen: set[str] = set()
        for r in results:
            task = tasks.get(r.task_id)
            problem = _problem(r, task, seen)
            seen.add(r.task_id)
            if problem or task is None:
                out.rejected.append(Rejected(r.task_id, problem or "Unknown task_id."))
                log.warning("Rejected agent result for task %s: %s", r.task_id, problem)
                continue
            row = task.product_url
            note = (r.note or "").strip() or None
            out.accepted += 1

            if r.status == "ok":
                assert r.price is not None
                last = latest.get(row.id)
                change = (
                    (r.price - last.current_price) / last.current_price
                    if last and last.current_price > 0
                    else None
                )
                flagged = change is not None and abs(change) > REVIEW_CHANGE
                member = r.member_price
                if member is not None and member >= r.price:
                    out.warnings.append(
                        f"{task.id}: member_price {member} is not below price {r.price}; ignored."
                    )
                    member = None
                s.add(
                    PriceRecord(
                        product_url_id=row.id,
                        current_price=r.price,
                        original_price=r.original_price or r.price,
                        member_price=member,
                        in_stock=True,
                        crawled_at=now,
                        source="agent",
                        needs_review=flagged,
                        raw_text=(r.raw_text or "").strip() or None,
                    )
                )
                _mark_success(row, now, in_stock=True)
                _end_task(s, task, "done", now, "ok", note)
                if flagged and last and change is not None:
                    out.flagged.append(
                        Flagged(
                            task.id,
                            row.product.name,
                            task.supermarket_id,
                            r.price,
                            last.current_price,
                            round(change * 100, 1),
                        )
                    )
                    log.warning(
                        "Flagged price for review: task %s %s at %s: %d (last %d, %+.1f%%)",
                        task.id,
                        row.product.name,
                        task.supermarket_id,
                        r.price,
                        last.current_price,
                        change * 100,
                    )
            elif r.status == "out_of_stock":
                _mark_success(row, now, in_stock=False)
                stock_note = "Out of stock" + (f": {note}" if note else "")
                _end_task(s, task, "done", now, "ok", stock_note)
            elif r.status == "not_found":
                reason = "Page not found" + (f": {note}" if note else " or product removed")
                _mark_failure(row, now, reason)
                _end_task(s, task, "failed", now, "failed", reason)
            else:  # error
                reason = note or "The agent could not read the price"
                if task.attempts < settings.agent_task_max_attempts:
                    task.status = "pending"
                    task.lease_expires_at = None
                    task.last_error = reason
                    out.requeued += 1
                else:
                    _mark_failure(row, now, reason)
                    _end_task(s, task, "failed", now, "failed", reason)
            if task.run_id is not None and task.status in FINISHED_STATUSES:
                runs.add(task.run_id)

        await s.flush()
        for run_id in sorted(runs):
            if await _finish_run_if_done(s, run_id, now):
                out.finished_runs.append(run_id)
        out.remaining_pending = await _count_pending(s)
    log.info(
        "Agent results: %d accepted (%d flagged, %d re-queued), %d rejected; %d pending",
        out.accepted,
        len(out.flagged),
        out.requeued,
        len(out.rejected),
        out.remaining_pending,
    )
    return out
