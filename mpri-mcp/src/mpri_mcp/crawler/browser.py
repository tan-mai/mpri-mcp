"""Playwright browser management: one shared Chromium, an isolated context per crawl run."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlparse

from playwright.async_api import (
    Browser,
    BrowserContext,
    Playwright,
    Request,
    Response,
    Route,
    async_playwright,
)
from playwright.async_api import Error as PlaywrightError
from playwright.async_api import TimeoutError as PlaywrightTimeoutError

from mpri_mcp.config import Settings
from mpri_mcp.crawler.config import CrawlConfig

log = logging.getLogger(__name__)

# Runs in the page. Uses innerText (respects CSS visibility and keeps "65,000 ₫" on one line,
# unlike parsing the raw HTML which also contains hidden menus and dialogs).
EXTRACT_JS = r"""(textSelector) => {
  const clean = s => (s || '').replace(/\s+/g, ' ').trim();
  const jsonld = [...document.querySelectorAll('script[type="application/ld+json"]')]
    .map(s => s.textContent || '').filter(Boolean);
  const meta = {};
  for (const m of document.querySelectorAll('meta[property], meta[itemprop], meta[name]')) {
    const key = m.getAttribute('property') || m.getAttribute('itemprop') || m.getAttribute('name');
    if (key && m.content && /price|currency|availability|^og:title$|^product:/i.test(key)) {
      meta[key] = m.content.slice(0, 200);
    }
  }
  const hints = [];
  const seen = new Set();
  const nodes = document.querySelectorAll(
    '[class*="price" i], [id*="price" i], [itemprop="price"], ' +
    '[data-price-amount], [data-price-type]');
  for (const el of nodes) {
    if (hints.length >= 15) break;
    const text = clean(el.innerText || el.getAttribute('content'));
    if (!text || text.length > 80 || !/\d/.test(text)) continue;
    const label = [...el.classList].filter(c => /price/i.test(c))
      .map(c => c.replace(/_{2,}[A-Za-z0-9-]{4,}$/, '')).join(' ')
      || el.getAttribute('itemprop') || el.getAttribute('data-price-type') || el.id || '';
    const key = label + '|' + text;
    if (seen.has(key)) continue;
    seen.add(key);
    hints.push({label: label.slice(0, 80), text, amount: el.getAttribute('data-price-amount')});
  }
  const root = (textSelector && document.querySelector(textSelector)) || document.body;
  return {
    title: document.title || '',
    h1: clean(document.querySelector('h1') && document.querySelector('h1').innerText),
    jsonld, meta, hints,
    text: root ? root.innerText : '',
  };
}"""

_BLOCKED_RESOURCES = {"image", "media", "font"}
# After the awaited API call has answered: time for the page to render its data, unless a
# wait_for_selector says exactly what to wait for.
_API_RENDER_MS = 1000


class FetchError(Exception):
    pass


@dataclass
class FetchedPage:
    url: str
    final_url: str
    status: int | None
    raw: dict[str, Any]


async def _block_heavy(route: Route) -> None:
    if route.request.resource_type in _BLOCKED_RESOURCES:
        await route.abort()
    else:
        await route.continue_()


class BrowserManager:
    def __init__(self, settings: Settings) -> None:
        self._settings = settings
        self._playwright: Playwright | None = None
        self._browser: Browser | None = None
        self._lock = asyncio.Lock()

    async def _ensure_browser(self) -> Browser:
        async with self._lock:
            if self._browser is None or not self._browser.is_connected():
                if self._playwright is None:
                    self._playwright = await async_playwright().start()
                log.info("Launching Chromium")
                self._browser = await self._playwright.chromium.launch(
                    headless=self._settings.browser_headless,
                    args=["--disable-dev-shm-usage"],
                )
            return self._browser

    async def stop(self) -> None:
        async with self._lock:
            if self._browser is not None:
                try:
                    await self._browser.close()
                except Exception:  # browser may already be gone
                    log.debug("Browser close failed", exc_info=True)
                self._browser = None
            if self._playwright is not None:
                await self._playwright.stop()
                self._playwright = None

    @asynccontextmanager
    async def context(self, config: CrawlConfig, base_url: str) -> AsyncIterator[BrowserContext]:
        browser = await self._ensure_browser()
        headers = {"Accept-Language": "vi-VN,vi;q=0.9,en;q=0.8", **config.extra_headers}
        ctx = await browser.new_context(
            locale="vi-VN",
            timezone_id=self._settings.scheduler_timezone,
            user_agent=self._settings.browser_user_agent,
            viewport={"width": 1366, "height": 900},
            extra_http_headers=headers,
            proxy=config.proxy.model_dump(exclude_none=True) if config.proxy else None,
        )
        try:
            if config.cookies:
                host = urlparse(base_url).netloc
                await ctx.add_cookies(
                    [
                        {
                            "name": c.name,
                            "value": c.value,
                            "domain": c.domain or host,
                            "path": c.path,
                        }
                        for c in config.cookies
                    ]
                )
            if config.block_resources:
                await ctx.route("**/*", _block_heavy)
            yield ctx
        finally:
            await ctx.close()

    async def run_setup(
        self, ctx: BrowserContext, config: CrawlConfig, base_url: str, timeout_s: float
    ) -> None:
        """Run the market's setup_actions once (e.g. pick a store) before visiting products."""
        timeout_ms = int(timeout_s * 1000)
        page = await ctx.new_page()
        try:
            await page.goto(
                config.setup_url or base_url, wait_until="domcontentloaded", timeout=timeout_ms
            )
            for step in config.setup_actions:
                if step.action == "goto" and step.url:
                    await page.goto(step.url, wait_until="domcontentloaded", timeout=timeout_ms)
                elif step.action == "click" and step.selector:
                    await page.click(step.selector, timeout=timeout_ms)
                elif step.action == "fill" and step.selector:
                    await page.fill(step.selector, step.value or "", timeout=timeout_ms)
                elif step.action == "press" and step.selector:
                    await page.press(step.selector, step.value or "Enter", timeout=timeout_ms)
                elif step.action == "wait_for" and step.selector:
                    await page.wait_for_selector(step.selector, timeout=timeout_ms)
                elif step.action == "wait":
                    await page.wait_for_timeout(step.ms or 1000)
        finally:
            await page.close()

    async def fetch(
        self,
        ctx: BrowserContext,
        url: str,
        config: CrawlConfig,
        timeout_s: float,
        wait_for_api: str | None = None,
    ) -> FetchedPage:
        """Load a page and read it. With wait_for_api (part of an API URL, from the
        supermarket's settings) the page is read once that call has answered."""
        if wait_for_api:
            return await self._fetch_after_api(ctx, url, config, timeout_s, wait_for_api)
        timeout_ms = int(timeout_s * 1000)
        page = await ctx.new_page()
        try:
            try:
                resp = await page.goto(url, wait_until="domcontentloaded", timeout=timeout_ms)
            except PlaywrightTimeoutError as e:
                raise FetchError(f"Page load timed out after {timeout_s:.0f}s") from e
            status = resp.status if resp else None
            if status is not None and status >= 400:
                raise FetchError(f"HTTP {status}")
            if config.wait_for_selector:
                try:
                    await page.wait_for_selector(config.wait_for_selector, timeout=timeout_ms)
                except PlaywrightTimeoutError as e:
                    raise FetchError(f"Selector '{config.wait_for_selector}' did not appear") from e
            else:
                try:
                    await page.wait_for_load_state("networkidle", timeout=min(10_000, timeout_ms))
                except PlaywrightTimeoutError:
                    pass  # busy pages (analytics, chat widgets) never go idle; content is there
            if config.wait_ms:
                await page.wait_for_timeout(config.wait_ms)
            raw = await page.evaluate(EXTRACT_JS, config.text_selector)
            return FetchedPage(url=url, final_url=page.url, status=status, raw=raw)
        finally:
            await page.close()

    async def _fetch_after_api(
        self, ctx: BrowserContext, url: str, config: CrawlConfig, timeout_s: float, api: str
    ) -> FetchedPage:
        """Wait for the page's own API call (e.g. its price request) instead of networkidle,
        which fires too early on single-page apps. A failed or blocked call fails the fetch
        right away with the network error. One deadline covers loading, the API call and the
        optional selector."""
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout_s

        def remaining_s() -> float:
            return max(0.001, deadline - loop.time())

        page = await ctx.new_page()
        answered: asyncio.Future[Response] = loop.create_future()

        def on_response(resp: Response) -> None:
            if api in resp.url and resp.request.method != "OPTIONS" and not answered.done():
                answered.set_result(resp)

        def on_failed(req: Request) -> None:
            # The page itself may cancel a call and send it again: keep waiting for that one.
            if api in req.url and req.failure != "net::ERR_ABORTED" and not answered.done():
                answered.set_exception(FetchError(f"API '{api}' failed: {req.failure}"))

        page.on("response", on_response)
        page.on("requestfailed", on_failed)
        try:
            try:
                resp = await page.goto(
                    url, wait_until="domcontentloaded", timeout=remaining_s() * 1000
                )
            except PlaywrightTimeoutError as e:
                raise FetchError(f"Page load timed out after {timeout_s:.0f}s") from e
            status = resp.status if resp else None
            if status is not None and status >= 400:
                raise FetchError(f"HTTP {status}")
            try:
                api_resp = await asyncio.wait_for(answered, remaining_s())
                if api_resp.status >= 400:
                    raise FetchError(f"API '{api}' returned HTTP {api_resp.status}")
                if api_resp.ok:  # wait for the whole body (redirects have none)
                    await asyncio.wait_for(api_resp.body(), remaining_s())
            except TimeoutError as e:
                raise FetchError(f"API '{api}' did not respond within {timeout_s:.0f}s") from e
            except PlaywrightError as e:
                raise FetchError(f"API '{api}' failed: {e.message.splitlines()[0]}") from e
            if config.wait_for_selector:
                try:
                    await page.wait_for_selector(
                        config.wait_for_selector, timeout=remaining_s() * 1000
                    )
                except PlaywrightTimeoutError as e:
                    raise FetchError(f"Selector '{config.wait_for_selector}' did not appear") from e
            else:
                await page.wait_for_timeout(_API_RENDER_MS)
            if config.wait_ms:
                await page.wait_for_timeout(config.wait_ms)
            raw = await page.evaluate(EXTRACT_JS, config.text_selector)
            return FetchedPage(url=url, final_url=page.url, status=status, raw=raw)
        finally:
            if answered.done() and not answered.cancelled():
                answered.exception()  # retrieved, so asyncio doesn't log it as unhandled
            else:
                answered.cancel()
            await page.close()
