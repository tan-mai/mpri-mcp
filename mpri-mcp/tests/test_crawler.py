import asyncio
import json
from contextlib import asynccontextmanager
from pathlib import Path

import pytest
from sqlalchemy import select
from tenacity import wait_none

from mpri_mcp.ai.base import ProviderError
from mpri_mcp.ai.registry import AIChoice, ProviderRegistry
from mpri_mcp.crawler.browser import FetchedPage, FetchError
from mpri_mcp.crawler.digest import build_digest, focus_text, jsonld_products
from mpri_mcp.crawler.extractor import (
    ExtractionRejected,
    PriceExtraction,
    PriceExtractor,
    validate_extraction,
)
from mpri_mcp.crawler.runner import CrawlRunner
from mpri_mcp.db.models import CrawlRun, CrawlRunItem, PriceRecord, ProductUrl
from mpri_mcp.services import products, supermarkets
from mpri_mcp.services.products import UrlInput
from tests.fakes import FakeProvider

FIXTURES = Path(__file__).parent / "fixtures"


def fixture(name: str) -> dict:
    return json.loads((FIXTURES / f"{name}.json").read_text(encoding="utf-8"))


URL_500 = fixture("mm_satori_500ml")["url"]
URL_350 = fixture("mm_satori_350ml")["url"]


# --------------------------------------------------------------------------- digest


def test_digest_of_discounted_page():
    fx = fixture("mm_satori_350ml")
    digest = build_digest(fx["url"], fx["raw"])
    assert digest.h1 == "Nước tinh khiết Satori, thùng 24 chai, 350ml"
    assert digest.products[0]["offers"]["price"] == 65000
    hints = "\n".join(digest.hints)
    assert "oldPrice: 89,000 ₫" in hints
    assert "MCardPrice: 58,998 ₫" in hints
    # Text starts near the product and stops before the recommendations block.
    assert digest.text.count("Satori") >= 1
    assert "65,000 ₫" in digest.text
    assert "Có thể bạn sẽ thích" not in digest.text
    assert "Sườn bò" not in digest.text
    assert "Bạn muốn giao đến địa chỉ nào" not in digest.text
    prompt = digest.to_prompt()
    assert prompt.startswith(f"URL: {fx['url']}")
    assert len(prompt) < 4000


def test_focus_text_truncates_and_handles_missing_h1():
    text = "\n".join(f"line {i}" for i in range(2000))
    out = focus_text(text, "", max_chars=500)
    assert len(out) <= 520 and out.endswith("[…truncated]")


def test_jsonld_graph_and_garbage():
    blocks = [
        "not json",
        json.dumps(
            {
                "@graph": [
                    {"@type": "WebPage"},
                    {
                        "@type": ["Product"],
                        "name": "X",
                        "offers": [{"price": "10", "priceCurrency": "VND", "seller": "ignored"}],
                    },
                ]
            }
        ),
    ]
    products_ = jsonld_products(blocks)
    assert products_ == [{"name": "X", "offers": [{"price": "10", "priceCurrency": "VND"}]}]


# --------------------------------------------------------------------------- extraction


def _ex(**kw) -> PriceExtraction:
    base = dict(
        is_product_page=True,
        product_name="x",
        current_price=65000,
        original_price=89000,
        member_price=58998,
        in_stock=True,
        unit="Thùng",
    )
    return PriceExtraction(**{**base, **kw})


def test_validate_extraction_happy_path():
    clean = validate_extraction(_ex())
    assert (clean.current_price, clean.original_price, clean.member_price) == (65000, 89000, 58998)


@pytest.mark.parametrize(
    "kw",
    [
        {"is_product_page": False},
        {"current_price": None},
        {"current_price": 0},
        {"current_price": 5_000_000_000},
    ],
)
def test_validate_extraction_rejects(kw):
    with pytest.raises(ValueError):
        validate_extraction(_ex(**kw))


def test_validate_extraction_repairs_inconsistent_values():
    clean = validate_extraction(_ex(original_price=None, member_price=70000))
    assert clean.original_price == 65000 and clean.member_price is None
    clean = validate_extraction(_ex(original_price=60000))
    assert clean.original_price == 65000 and clean.notes


def satori_responder(system: str, prompt: str, schema):
    if "350ml" in prompt:
        return _ex()
    return _ex(current_price=109000, original_price=None, member_price=None)


def _registry(settings, provider):
    return ProviderRegistry(settings, providers={"fake": provider}, retry_wait=wait_none())


async def test_extractor_sends_digest_and_rejects_bad_answers(settings):
    fx = fixture("mm_satori_350ml")
    digest = build_digest(fx["url"], fx["raw"])
    provider = FakeProvider(responder=satori_responder)
    extractor = PriceExtractor(_registry(settings, provider))
    outcome = await extractor.extract(digest, AIChoice("fake", "m"))
    assert outcome.price.member_price == 58998
    assert "MCardPrice" in provider.calls[0]["prompt"]

    bad = FakeProvider(responder=lambda *_: _ex(is_product_page=False))
    with pytest.raises(ExtractionRejected) as exc:
        await PriceExtractor(_registry(settings, bad)).extract(digest, AIChoice("fake", "m"))
    assert exc.value.result.input_tokens == 100


# --------------------------------------------------------------------------- runner


class FakeBrowser:
    def __init__(self, pages: dict, delay: float = 0) -> None:
        self.pages = pages
        self.delay = delay
        self.contexts = 0

    @asynccontextmanager
    async def context(self, config, base_url):
        self.contexts += 1
        yield object()

    async def run_setup(self, ctx, config, base_url, timeout_s):
        pass

    async def fetch(self, ctx, url, config, timeout_s):
        if self.delay:
            await asyncio.sleep(self.delay)
        item = self.pages[url]
        if isinstance(item, Exception):
            raise item
        return FetchedPage(url=url, final_url=url, status=200, raw=item)

    async def stop(self):
        pass


async def _market_with_urls(db, extra_url: str | None = None):
    mm = await supermarkets.add_supermarket(db, "MM Mega Market", "online.mmvietnam.com")
    await products.add_product_with_urls(db, "Satori 500ml", None, [UrlInput(mm.id, URL_500)])
    await products.add_product_with_urls(db, "Satori 350ml", None, [UrlInput(mm.id, URL_350)])
    if extra_url:
        await products.add_product_with_urls(db, "Broken", None, [UrlInput(mm.id, extra_url)])
    return mm


def _runner(settings, db, browser, provider):
    registry = _registry(settings, provider)
    return CrawlRunner(settings, db, browser, PriceExtractor(registry), registry)


async def test_crawl_saves_prices_and_isolates_failures(settings, db):
    broken = "https://online.mmvietnam.com/product/gone.html"
    mm = await _market_with_urls(db, broken)
    browser = FakeBrowser(
        {
            URL_500: fixture("mm_satori_500ml")["raw"],
            URL_350: fixture("mm_satori_350ml")["raw"],
            broken: FetchError("HTTP 404"),
        }
    )
    runner = _runner(settings, db, browser, FakeProvider(responder=satori_responder))
    run_id = await runner.crawl_supermarket(mm.id, trigger="manual")

    async with db.session() as s:
        run = await s.get(CrawlRun, run_id)
        records = {
            url: rec
            for rec, url in (
                await s.execute(select(PriceRecord, ProductUrl.url).join(ProductUrl))
            ).all()
        }
        items = (await s.scalars(select(CrawlRunItem))).all()
        broken_row = await s.scalar(select(ProductUrl).where(ProductUrl.url == broken))

    assert (run.status, run.urls_total, run.urls_ok, run.urls_failed) == ("partial", 3, 2, 1)
    assert run.finished_at is not None
    assert records[URL_350].current_price == 65000
    assert records[URL_350].member_price == 58998
    assert records[URL_500].original_price == 109000
    assert records[URL_350].provider == "fake"
    failed = [i for i in items if i.status == "failed"]
    assert failed[0].error == "HTTP 404"
    assert sum(i.input_tokens or 0 for i in items) == 200
    assert broken_row.consecutive_failures == 1 and broken_row.last_error == "HTTP 404"
    assert browser.contexts == 1


async def test_crawl_marks_failed_when_provider_down(settings, db):
    mm = await _market_with_urls(db)
    browser = FakeBrowser(
        {URL_500: fixture("mm_satori_500ml")["raw"], URL_350: fixture("mm_satori_350ml")["raw"]}
    )
    provider = FakeProvider(errors=[ProviderError("quota", retryable=False)] * 2)
    run_id = await _runner(settings, db, browser, provider).crawl_supermarket(mm.id)
    async with db.session() as s:
        run = await s.get(CrawlRun, run_id)
    assert (run.status, run.urls_ok, run.urls_failed) == ("failed", 0, 2)


async def test_crawl_job_timeout(settings, db):
    settings.job_timeout_seconds = 0.2
    mm = await _market_with_urls(db)
    browser = FakeBrowser(
        {URL_500: fixture("mm_satori_500ml")["raw"], URL_350: fixture("mm_satori_350ml")["raw"]},
        delay=5,
    )
    run_id = await _runner(settings, db, browser, FakeProvider(satori_responder)).crawl_supermarket(
        mm.id
    )
    async with db.session() as s:
        run = await s.get(CrawlRun, run_id)
    assert run.status == "timeout" and "JOB_TIMEOUT_SECONDS" in run.error


async def test_dry_run_does_not_save(settings, db):
    mm = await _market_with_urls(db)
    browser = FakeBrowser({URL_350: fixture("mm_satori_350ml")["raw"]})
    runner = _runner(settings, db, browser, FakeProvider(responder=satori_responder))
    result = await runner.dry_run(URL_350, supermarket_id=mm.id)
    assert result.ok and result.price["member_price"] == 58998
    assert result.provider == "fake" and result.model == "fake-model"
    async with db.session() as s:
        assert (await s.scalars(select(PriceRecord))).all() == []


async def test_market_ai_override_is_used(settings, db):
    mm = await _market_with_urls(db)
    await supermarkets.update_supermarket(db, mm.id, ai_model="special-model")
    browser = FakeBrowser({URL_350: fixture("mm_satori_350ml")["raw"]})
    provider = FakeProvider(responder=satori_responder)
    runner = _runner(settings, db, browser, provider)
    result = await runner.dry_run(URL_350, supermarket_id=mm.id)
    assert result.model == "special-model"
