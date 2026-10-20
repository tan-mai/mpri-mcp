from datetime import timedelta

import pytest

from mpri_mcp.services import analytics, products, supermarkets
from mpri_mcp.services.errors import ConflictError, NotFoundError, ValidationError
from mpri_mcp.services.products import UrlInput
from mpri_mcp.utils.time import utcnow
from tests.conftest import add_price

MM_500 = "https://online.mmvietnam.com/product/nuoc-tinh-khiet-satori-thung-24-chai-500ml.html"
MM_350 = "https://online.mmvietnam.com/product/nuoc-tinh-khiet-satori-thung-24-chai-350ml.html"


async def _setup(db):
    mm = await supermarkets.add_supermarket(db, "MM Mega Market", "online.mmvietnam.com")
    wm = await supermarkets.add_supermarket(db, "Winmart", "https://winmart.vn")
    return mm, wm


async def test_add_supermarket_slug_and_normalized_url(db):
    market = await supermarkets.add_supermarket(db, "Lotte Mart", "lottemart.vn")
    assert market.id == "lotte-mart"
    assert market.base_url == "https://lottemart.vn"


async def test_add_supermarket_duplicate_name(db):
    await supermarkets.add_supermarket(db, "Lotte Mart", "lottemart.vn")
    with pytest.raises(ConflictError):
        await supermarkets.add_supermarket(db, "lotte  MART", "lotte.vn")


async def test_add_supermarket_invalid_url(db):
    with pytest.raises(ValidationError):
        await supermarkets.add_supermarket(db, "X", "not a url")


async def test_update_supermarket_crawl_config_validated(db):
    mm, _ = await _setup(db)
    updated = await supermarkets.update_supermarket(
        db, mm.id, crawl_config={"wait_for_selector": ".price", "wait_ms": 500}
    )
    assert updated.crawl_config == {"wait_for_selector": ".price", "wait_ms": 500}
    with pytest.raises(ValidationError):
        await supermarkets.update_supermarket(db, mm.id, crawl_config={"bogus": 1})


async def test_add_product_with_urls(db):
    mm, wm = await _setup(db)
    result = await products.add_product_with_urls(
        db,
        "Nước tinh khiết Satori thùng 24 chai 500ml",
        "Nước uống",
        [
            UrlInput(mm.id, MM_500),
            UrlInput(wm.id, "https://shopee.vn/satori"),
            UrlInput("nope", "https://x.vn/a"),
        ],
    )
    assert result.created
    statuses = {r.supermarket_id: r for r in result.urls}
    assert statuses[mm.id].status == "added" and statuses[mm.id].warning is None
    assert statuses[wm.id].status == "added" and "does not match" in statuses[wm.id].warning
    assert statuses["nope"].status == "error"

    # Same product name (different accents/case) -> reuse product, update URL.
    again = await products.add_product_with_urls(
        db, "nuoc tinh khiet satori thung 24 chai 500ml", None, [UrlInput(mm.id, MM_500 + "?v=2")]
    )
    assert not again.created
    assert again.product.id == result.product.id
    assert again.urls[0].status == "updated"


async def test_product_name_required(db):
    with pytest.raises(ValidationError):
        await products.add_product_with_urls(db, "   ", None, [])


async def test_price_comparison_and_alternatives(db):
    mm, wm = await _setup(db)
    p500 = await products.add_product_with_urls(
        db,
        "Satori 24 chai 500ml",
        None,
        [UrlInput(mm.id, MM_500), UrlInput(wm.id, "https://winmart.vn/s500")],
    )
    await products.add_product_with_urls(
        db, "Satori 24 chai 350ml", None, [UrlInput(mm.id, MM_350)]
    )
    full = await products.get_product(db, p500.product.id)
    url_by_market = {u.supermarket_id: u.id for u in full.urls}
    await add_price(db, url_by_market[mm.id], 95_000, original=110_000, member=90_000)
    await add_price(db, url_by_market[wm.id], 99_000)

    cmp = await analytics.price_comparison(db, "satori 500ml")
    assert cmp.product.id == p500.product.id
    assert cmp.cheapest.supermarket_id == mm.id
    assert cmp.cheapest.discount_pct == pytest.approx(13.6)
    assert cmp.cheapest.member_discount_pct == pytest.approx(18.2)
    assert [p.current_price for p in cmp.prices] == [95_000, 99_000]

    # "satori 24 chai" matches both products equally -> the other shows as an alternative.
    ambiguous = await analytics.price_comparison(db, "satori 24 chai")
    assert ambiguous.alternatives


async def test_price_comparison_unknown_product(db):
    with pytest.raises(NotFoundError):
        await analytics.price_comparison(db, "bia tiger")


async def test_latest_price_wins(db):
    mm, _ = await _setup(db)
    p = await products.add_product_with_urls(db, "Satori 500ml", None, [UrlInput(mm.id, MM_500)])
    url_id = (await products.get_product(db, p.product.id)).urls[0].id
    await add_price(db, url_id, 100_000, at=utcnow() - timedelta(days=2))
    await add_price(db, url_id, 90_000, at=utcnow() - timedelta(hours=1))
    cmp = await analytics.price_comparison(db, "satori 500ml")
    assert cmp.prices[0].current_price == 90_000
    assert not cmp.prices[0].is_stale


async def test_deep_discounts(db):
    mm, wm = await _setup(db)
    a = await products.add_product_with_urls(db, "Satori 500ml", None, [UrlInput(mm.id, MM_500)])
    b = await products.add_product_with_urls(db, "Satori 350ml", None, [UrlInput(mm.id, MM_350)])
    c = await products.add_product_with_urls(
        db, "Old deal", None, [UrlInput(wm.id, "https://winmart.vn/o")]
    )
    ua = (await products.get_product(db, a.product.id)).urls[0].id
    ub = (await products.get_product(db, b.product.id)).urls[0].id
    uc = (await products.get_product(db, c.product.id)).urls[0].id
    await add_price(db, ua, 70_000, original=100_000)  # 30%
    await add_price(db, ub, 95_000, original=100_000)  # 5%
    await add_price(db, uc, 10_000, original=100_000, at=utcnow() - timedelta(days=30))  # too old

    items = await analytics.deep_discounts(db, 20, days=7)
    assert [i.product_name for i in items] == ["Satori 500ml"]
    assert items[0].discount_pct == 30.0

    with pytest.raises(ValidationError):
        await analytics.deep_discounts(db, 0)


async def test_price_history_daily_last_value(db):
    mm, _ = await _setup(db)
    p = await products.add_product_with_urls(db, "Satori 500ml", None, [UrlInput(mm.id, MM_500)])
    url_id = (await products.get_product(db, p.product.id)).urls[0].id
    now = utcnow()
    await add_price(db, url_id, 100_000, at=now - timedelta(days=3))
    await add_price(db, url_id, 80_000, at=now - timedelta(days=2))
    await add_price(db, url_id, 90_000, at=now - timedelta(minutes=5))
    hist = await analytics.price_history(db, "satori", days=30)
    series = hist.series[0]
    assert len(series.points) == 3
    assert series.min_point.current_price == 80_000
    assert series.max_price == 100_000
    assert series.latest.current_price == 90_000


async def test_delete_supermarket_cascades(db):
    mm, _ = await _setup(db)
    p = await products.add_product_with_urls(db, "Satori 500ml", None, [UrlInput(mm.id, MM_500)])
    url_id = (await products.get_product(db, p.product.id)).urls[0].id
    await add_price(db, url_id, 1000)
    await supermarkets.delete_supermarket(db, mm.id)
    assert (await products.get_product(db, p.product.id)).urls == []


async def test_list_supermarkets_counts(db):
    mm, wm = await _setup(db)
    await products.add_product_with_urls(db, "Satori 500ml", None, [UrlInput(mm.id, MM_500)])
    summaries = {s.supermarket.id: s for s in await supermarkets.list_supermarkets(db)}
    assert summaries[mm.id].url_count == 1
    assert summaries[wm.id].url_count == 0
    assert summaries[mm.id].last_run is None
