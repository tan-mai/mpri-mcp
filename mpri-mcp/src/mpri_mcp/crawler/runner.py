"""The body of each supermarket's crawl job, plus single-URL dry runs."""

from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass, field
from typing import Any

from playwright.async_api import BrowserContext
from playwright.async_api import Error as PlaywrightError
from sqlalchemy import func, select

from mpri_mcp.ai.base import ProviderError
from mpri_mcp.ai.registry import AIChoice, ProviderRegistry
from mpri_mcp.config import Settings
from mpri_mcp.crawler.browser import BrowserManager, FetchError
from mpri_mcp.crawler.config import CrawlConfig
from mpri_mcp.crawler.digest import PageDigest, build_digest
from mpri_mcp.crawler.extractor import ExtractionRejected, PriceExtractor
from mpri_mcp.db import Database
from mpri_mcp.db.models import CrawlRun, CrawlRunItem, PriceRecord, ProductUrl, Supermarket
from mpri_mcp.services.settings import get_ai_settings
from mpri_mcp.services.supermarkets import get_supermarket
from mpri_mcp.utils.time import utcnow

log = logging.getLogger(__name__)

_EXPECTED_ERRORS = (FetchError, ProviderError, ExtractionRejected, PlaywrightError, TimeoutError)


@dataclass
class DryRunResult:
    url: str
    ok: bool
    digest: PageDigest | None = None
    extraction: dict[str, Any] | None = None
    price: dict[str, Any] | None = None
    provider: str | None = None
    model: str | None = None
    input_tokens: int | None = None
    output_tokens: int | None = None
    latency_ms: int | None = None
    error: str | None = None
    notes: list[str] = field(default_factory=list)


def _error_text(exc: BaseException) -> str:
    if isinstance(exc, TimeoutError):
        return "Timed out"
    msg = str(exc).strip().splitlines()[0] if str(exc).strip() else type(exc).__name__
    return msg[:500]


class CrawlRunner:
    def __init__(
        self,
        settings: Settings,
        db: Database,
        browser: BrowserManager,
        extractor: PriceExtractor,
        registry: ProviderRegistry,
    ) -> None:
        self._settings = settings
        self._db = db
        self._browser = browser
        self._extractor = extractor
        self._registry = registry
        self._market_slots = asyncio.Semaphore(max(1, settings.max_concurrent_markets))

    async def resolve_ai(
        self, market: Supermarket | None, provider: str | None = None, model: str | None = None
    ) -> tuple[AIChoice, AIChoice | None]:
        ai = await get_ai_settings(self._db)
        levels = [
            (provider, model),
            (market.ai_provider, market.ai_model) if market else (None, None),
            (ai.provider, ai.model),
            (self._settings.ai_provider, self._settings.ai_model),
        ]
        choice = self._registry.resolve(levels)
        fallback = self._registry.resolve_fallback(
            [
                (ai.fallback_provider, ai.fallback_model),
                (self._settings.ai_fallback_provider, self._settings.ai_fallback_model),
            ]
        )
        return choice, fallback

    # ------------------------------------------------------------------ scheduled crawl
    async def crawl_supermarket(self, supermarket_id: str, trigger: str = "cron") -> int:
        async with self._market_slots:
            market = await get_supermarket(self._db, supermarket_id)
            async with self._db.session() as s:
                urls = list(
                    (
                        await s.scalars(
                            select(ProductUrl).where(
                                ProductUrl.supermarket_id == supermarket_id, ProductUrl.is_active
                            )
                        )
                    ).all()
                )
                run = CrawlRun(
                    supermarket_id=supermarket_id,
                    trigger=trigger,
                    status="running",
                    urls_total=len(urls),
                )
                s.add(run)
            log.info("Crawl #%s of %s started (%d URLs, %s)", run.id, market.id, len(urls), trigger)

            status, error = "success", None
            try:
                await asyncio.wait_for(
                    self._crawl_urls(run.id, market, urls),
                    timeout=self._settings.job_timeout_seconds,
                )
            except TimeoutError:
                status, error = (
                    "timeout",
                    (f"Run exceeded JOB_TIMEOUT_SECONDS={self._settings.job_timeout_seconds}"),
                )
            except asyncio.CancelledError:
                status, error = "failed", "Cancelled (server shutting down)"
                raise
            except Exception as e:
                log.exception("Crawl #%s of %s crashed", run.id, market.id)
                status, error = "failed", _error_text(e)
            finally:
                await self._finish_run(run.id, status, error)
            return run.id

    async def _finish_run(self, run_id: int, status: str, error: str | None) -> None:
        async with self._db.session() as s:
            run = await s.get(CrawlRun, run_id)
            assert run is not None
            counts = dict(
                (
                    await s.execute(
                        select(CrawlRunItem.status, func.count())
                        .where(CrawlRunItem.run_id == run_id)
                        .group_by(CrawlRunItem.status)
                    )
                ).all()
            )
            run.urls_ok = counts.get("ok", 0)
            run.urls_failed = counts.get("failed", 0)
            if status == "success" and run.urls_failed:
                status = "partial" if run.urls_ok else "failed"
            run.status, run.error, run.finished_at = status, error, utcnow()
        log.info(
            "Crawl #%s finished: %s (%d ok, %d failed)",
            run_id,
            status,
            run.urls_ok,
            run.urls_failed,
        )

    async def _crawl_urls(self, run_id: int, market: Supermarket, urls: list[ProductUrl]) -> None:
        if not urls:
            return
        config = CrawlConfig.model_validate(market.crawl_config or {})
        choice, fallback = await self.resolve_ai(market)
        timeout = self._settings.page_timeout_seconds
        async with self._browser.context(config, market.base_url) as ctx:
            if config.setup_actions:
                await self._browser.run_setup(ctx, config, market.base_url, timeout)
            pages = asyncio.Semaphore(max(1, self._settings.max_pages_per_market))

            async def one(url_row: ProductUrl) -> None:
                async with pages:
                    await self._crawl_url(run_id, ctx, config, url_row, choice, fallback)

            await asyncio.gather(*(one(u) for u in urls))

    async def _crawl_url(
        self,
        run_id: int,
        ctx: BrowserContext,
        config: CrawlConfig,
        url_row: ProductUrl,
        choice: AIChoice,
        fallback: AIChoice | None,
    ) -> None:
        started = time.monotonic()
        item = CrawlRunItem(run_id=run_id, product_url_id=url_row.id, url=url_row.url, status="ok")
        record: PriceRecord | None = None
        try:
            page = await asyncio.wait_for(
                self._browser.fetch(ctx, url_row.url, config, self._settings.page_timeout_seconds),
                timeout=self._settings.page_timeout_seconds + 20,
            )
            digest = build_digest(
                url_row.url, page.raw, config.max_chars or self._settings.page_text_max_chars
            )
            outcome = await self._extractor.extract(digest, choice, fallback)
            res = outcome.result
            item.provider, item.model = res.provider, res.model
            item.input_tokens, item.output_tokens = res.input_tokens, res.output_tokens
            record = PriceRecord(
                product_url_id=url_row.id,
                current_price=outcome.price.current_price,
                original_price=outcome.price.original_price,
                member_price=outcome.price.member_price,
                in_stock=outcome.price.in_stock,
                provider=res.provider,
                model=res.model,
                raw_extraction=outcome.raw(),
            )
        except _EXPECTED_ERRORS as e:
            item.status, item.error = "failed", _error_text(e)
            if isinstance(e, ExtractionRejected):
                r = e.result
                item.provider, item.model = r.provider, r.model
                item.input_tokens, item.output_tokens = r.input_tokens, r.output_tokens
            log.warning("Crawl #%s: %s failed: %s", run_id, url_row.url, item.error)
        except Exception as e:
            item.status, item.error = "failed", f"Unexpected error: {_error_text(e)}"
            log.exception("Crawl #%s: %s crashed", run_id, url_row.url)
        item.duration_ms = int((time.monotonic() - started) * 1000)

        now = utcnow()
        async with self._db.session() as s:
            s.add(item)
            row = await s.get(ProductUrl, url_row.id)
            if row is None:  # deleted while crawling
                return
            row.last_crawled_at = now
            if record is not None:
                s.add(record)
                row.last_success_at = now
                row.last_error = None
                row.consecutive_failures = 0
            else:
                row.last_error = item.error
                row.consecutive_failures += 1

    # ------------------------------------------------------------------ dry run
    async def dry_run(
        self,
        url: str,
        *,
        supermarket_id: str | None = None,
        provider: str | None = None,
        model: str | None = None,
        extract: bool = True,
    ) -> DryRunResult:
        """Fetch (and optionally extract) one URL without saving anything."""
        market = await get_supermarket(self._db, supermarket_id) if supermarket_id else None
        config = CrawlConfig.model_validate(market.crawl_config if market else {})
        result = DryRunResult(url=url, ok=False)
        timeout = self._settings.page_timeout_seconds
        try:
            base = market.base_url if market else url
            async with self._browser.context(config, base) as ctx:
                if config.setup_actions:
                    await self._browser.run_setup(ctx, config, base, timeout)
                page = await self._browser.fetch(ctx, url, config, timeout)
            result.digest = build_digest(
                url, page.raw, config.max_chars or self._settings.page_text_max_chars
            )
            if not extract:
                result.ok = True
                return result
            choice, fallback = await self.resolve_ai(market, provider, model)
            outcome = await self._extractor.extract(result.digest, choice, fallback)
            res = outcome.result
            result.ok = True
            result.extraction = res.data.model_dump()
            result.price = {
                "current_price": outcome.price.current_price,
                "original_price": outcome.price.original_price,
                "member_price": outcome.price.member_price,
                "in_stock": outcome.price.in_stock,
            }
            result.notes = outcome.price.notes
            result.provider, result.model = res.provider, res.model
            result.input_tokens, result.output_tokens = res.input_tokens, res.output_tokens
            result.latency_ms = res.latency_ms
        except ExtractionRejected as e:
            r = e.result
            result.error = str(e)
            result.extraction = r.data.model_dump()
            result.provider, result.model = r.provider, r.model
            result.input_tokens, result.output_tokens = r.input_tokens, r.output_tokens
        except _EXPECTED_ERRORS as e:
            result.error = _error_text(e)
        return result
