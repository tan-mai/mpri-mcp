"""BrowserManager.fetch with a real headless Chromium, against pages served offline by routes.
Skipped when Chromium is not installed (`uv run playwright install --only-shell chromium`)."""

import asyncio
import time

import pytest
from playwright.async_api import Error as PlaywrightError
from playwright.async_api import Route

from mpri_mcp.crawler.browser import BrowserManager, FetchError
from mpri_mcp.crawler.config import CrawlConfig

SITE = "https://shop.test"
PAGE = f"{SITE}/nuoc-tang-luc/sting-24-lon"
API = "/gw/Product/GetProductDetail"

# Like the real sites: the price arrives from the API after the HTML has loaded.
PAGE_WITH_API = """<!doctype html><html><head><title>Sting 24 lon</title></head><body>
<h1>Sting 24 lon</h1><div id="box">Đang tải…</div>
<script>
setTimeout(() => fetch('/gw/Product/GetProductDetail?id=1')
  .then(r => r.json())
  .then(d => setTimeout(() => {
    document.getElementById('box').innerHTML = '<span class="product_price">' + d.price + '</span>';
  }, RENDER_MS))
  .catch(() => {}), 200);
</script></body></html>"""
PAGE_WITHOUT_API = "<!doctype html><html><body><h1>Sting 24 lon</h1></body></html>"


@pytest.fixture
async def browser(settings):
    manager = BrowserManager(settings)
    try:
        async with manager.context(CrawlConfig(), SITE):
            pass
    except PlaywrightError as e:
        await manager.stop()
        pytest.skip(f"Chromium is not available: {str(e).splitlines()[0]}")
    yield manager
    await manager.stop()


async def _fetch(browser, api_reply, *, html=None, render_ms=0, config=None, timeout_s=20.0):
    config = config or CrawlConfig()
    page_html = html or PAGE_WITH_API.replace("RENDER_MS", str(render_ms))

    async def serve(route: Route) -> None:
        if API in route.request.url:
            await api_reply(route)
        else:
            await route.fulfill(status=200, content_type="text/html; charset=utf-8", body=page_html)

    async with browser.context(config, SITE) as ctx:
        await ctx.route(f"{SITE}/**", serve)
        return await browser.fetch(ctx, PAGE, config, timeout_s, wait_for_api=API)


def _price_after(seconds: float, status: int = 200):
    async def reply(route: Route) -> None:
        await asyncio.sleep(seconds)
        await route.fulfill(status=status, json={"price": "204.000₫"})

    return reply


async def test_reads_the_page_once_the_api_has_answered(browser):
    page = await _fetch(browser, _price_after(1.5))
    assert "204.000₫" in page.raw["text"]


async def test_waits_for_the_selector_after_the_api(browser):
    config = CrawlConfig(wait_for_selector=".product_price")
    page = await _fetch(browser, _price_after(0.2), render_ms=1500, config=config)
    assert "204.000₫" in page.raw["text"]


async def test_blocked_api_fails_right_away_with_the_network_error(browser):
    async def reset(route: Route) -> None:
        await route.abort("connectionreset")

    started = time.monotonic()
    with pytest.raises(FetchError, match=r"API '/gw/Product/GetProductDetail' failed: net::ERR_"):
        await _fetch(browser, reset, timeout_s=30)
    assert time.monotonic() - started < 10


async def test_api_error_status_fails(browser):
    with pytest.raises(FetchError, match="returned HTTP 403"):
        await _fetch(browser, _price_after(0, status=403))


async def test_api_never_called_times_out(browser):
    with pytest.raises(FetchError, match="did not respond within 2s"):
        await _fetch(browser, _price_after(0), html=PAGE_WITHOUT_API, timeout_s=2)
