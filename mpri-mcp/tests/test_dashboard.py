"""Dashboard pages render and their forms work (with fake browser + AI provider)."""

import re

import pytest
from fastapi.testclient import TestClient
from tenacity import wait_none

from mpri_mcp.ai.registry import ProviderRegistry
from mpri_mcp.app import create_app
from mpri_mcp.container import Container
from tests.fakes import FakeProvider
from tests.test_admin_auth import csrf, do_setup
from tests.test_crawler import URL_350, URL_500, FakeBrowser, fixture, satori_responder


@pytest.fixture
def app(settings):
    registry = ProviderRegistry(
        settings,
        providers={"fake": FakeProvider(responder=satori_responder)},
        retry_wait=wait_none(),
    )
    browser = FakeBrowser(
        {URL_500: fixture("mm_satori_500ml")["raw"], URL_350: fixture("mm_satori_350ml")["raw"]}
    )
    return create_app(settings, Container(settings, registry=registry, browser=browser))


@pytest.fixture
def client(app):
    with TestClient(app, base_url="http://localhost:8000", follow_redirects=False) as c:
        do_setup(c)
        yield c


def post(client, page_url, target, **data):
    page = client.get(page_url)
    assert page.status_code == 200, page.text
    return client.post(target, data={"csrf_token": csrf(page.text), **data})


def test_empty_pages_render(client):
    for url in [
        "/admin",
        "/admin/supermarkets",
        "/admin/products",
        "/admin/runs",
        "/admin/ai",
        "/admin/apps",
        "/admin/account",
    ]:
        resp = client.get(url)
        assert resp.status_code == 200, (url, resp.text[:500])
        assert 'aria-current="page"' in resp.text
    assert "Connect Claude" in client.get("/admin").text
    assert "http://localhost:8000/mcp" in client.get("/admin/apps").text


def test_supermarket_and_product_workflow(client):
    resp = post(
        client,
        "/admin/supermarkets",
        "/admin/supermarkets",
        name="MM Mega Market",
        base_url="online.mmvietnam.com",
    )
    assert (
        resp.status_code == 303 and resp.headers["location"] == "/admin/supermarkets/mm-mega-market"
    )
    detail = client.get("/admin/supermarkets/mm-mega-market")
    assert "Added MM Mega Market" in detail.text and "no schedule" in detail.text

    preview = client.get("/admin/cron-preview", params={"cron_expression": "0 3 * * *"})
    assert "Next runs" in preview.text and "03:00" in preview.text
    bad = client.get("/admin/cron-preview", params={"cron_expression": "nope"})
    assert "Invalid cron expression" in bad.text

    resp = post(
        client,
        "/admin/supermarkets/mm-mega-market",
        "/admin/supermarkets/mm-mega-market/schedule",
        cron_expression="0 3 * * *",
        action="save",
    )
    assert resp.status_code == 303
    detail = client.get("/admin/supermarkets/mm-mega-market")
    assert "scheduled" in detail.text and "Next run" in detail.text

    resp = post(
        client,
        "/admin/products",
        "/admin/products",
        name="Nước tinh khiết Satori, thùng 24 chai, 350ml",
        category="Nước",
        **{"url__mm-mega-market": URL_350},
    )
    assert resp.status_code == 303
    product_url = resp.headers["location"]
    page = client.get(product_url)
    assert "Satori" in page.text and 'id="price-chart"' in page.text
    assert "/static/chart.umd.min.js" in page.text

    # Re-test one URL (htmx fragment) — fake browser + fake AI.
    url_id = re.search(r'hx-post="/admin/urls/(\d+)/test"', page.text).group(1)
    test = client.post(f"/admin/urls/{url_id}/test", headers={"X-CSRF-Token": csrf(page.text)})
    assert test.status_code == 200 and "65.000 ₫" in test.text and "58.998 ₫" in test.text

    # Crawl now, then the history JSON and the overview reflect it.
    resp = post(
        client, "/admin/supermarkets/mm-mega-market", "/admin/supermarkets/mm-mega-market/run"
    )
    assert resp.status_code == 303
    import time

    for _ in range(100):
        if "crawling" not in client.get("/admin/supermarkets/mm-mega-market").text:
            break
        time.sleep(0.02)
    history = client.get(f"{product_url}/history.json").json()
    assert history["series"][0]["name"] == "MM Mega Market"
    assert history["series"][0]["points"][0]["current"] == 65000
    overview = client.get("/admin")
    assert "success" in overview.text

    runs = client.get("/admin/runs")
    run_id = re.search(r'href="/admin/runs/(\d+)"', runs.text).group(1)
    detail = client.get(f"/admin/runs/{run_id}")
    assert "fake/fake-model" in detail.text


def test_crawl_config_validation(client):
    post(
        client, "/admin/supermarkets", "/admin/supermarkets", name="Winmart", base_url="winmart.vn"
    )
    resp = post(
        client,
        "/admin/supermarkets/winmart",
        "/admin/supermarkets/winmart/crawl-config",
        crawl_config='{"bogus": 1}',
    )
    assert resp.status_code == 303
    assert "Not saved" in client.get("/admin/supermarkets/winmart").text
    post(
        client,
        "/admin/supermarkets/winmart",
        "/admin/supermarkets/winmart/crawl-config",
        crawl_config='{"wait_for_selector": ".price"}',
    )
    assert "wait_for_selector" in client.get("/admin/supermarkets/winmart").text


def test_delete_supermarket_requires_typed_confirmation(client):
    post(
        client, "/admin/supermarkets", "/admin/supermarkets", name="Winmart", base_url="winmart.vn"
    )
    resp = post(
        client, "/admin/supermarkets/winmart", "/admin/supermarkets/winmart/delete", confirm="wrong"
    )
    assert resp.headers["location"] == "/admin/supermarkets/winmart"
    resp = post(
        client,
        "/admin/supermarkets/winmart",
        "/admin/supermarkets/winmart/delete",
        confirm="winmart",
    )
    assert resp.headers["location"] == "/admin/supermarkets"
    assert client.get("/admin/supermarkets/winmart").status_code == 404


def test_ai_settings_and_prices(client):
    resp = post(
        client,
        "/admin/ai",
        "/admin/ai",
        provider="fake",
        model="m2",
        fallback_provider="",
        fallback_model="",
    )
    assert resp.status_code == 303
    page = client.get("/admin/ai")
    assert "fake/m2" in page.text
    resp = post(client, "/admin/ai", "/admin/ai", provider="nope")
    assert "Unknown provider" in client.get("/admin/ai").text

    post(client, "/admin/ai", "/admin/ai/prices", prices='{"m2": {"input": 1, "output": 2}}')
    assert '"m2"' in client.get("/admin/ai").text
    post(client, "/admin/ai", "/admin/ai/prices", prices="not json")
    assert "Not saved" in client.get("/admin/ai").text


def test_ai_test_extraction_fragment(client):
    page = client.get("/admin/ai")
    resp = client.post("/admin/ai/test", data={"csrf_token": csrf(page.text), "url": URL_350})
    assert resp.status_code == 200 and "65.000 ₫" in resp.text
    resp = client.post("/admin/ai/test", data={"csrf_token": csrf(page.text), "url": "ftp://x"})
    assert "full http(s) URL" in resp.text


def test_pages_require_login(app):
    with TestClient(app, base_url="http://localhost:8000", follow_redirects=False) as c:
        do_setup(c)
        c.cookies.clear()
        for url in [
            "/admin/supermarkets",
            "/admin/ai",
            "/admin/apps",
            "/admin/runs/1",
            "/admin/products/1/history.json",
        ]:
            resp = c.get(url)
            assert resp.status_code == 303 and resp.headers["location"].startswith("/admin/login")


def test_unknown_ids_render_404(client):
    assert client.get("/admin/supermarkets/nope").status_code == 404
    assert client.get("/admin/products/999").status_code == 404
    assert client.get("/admin/runs/999").status_code == 404
