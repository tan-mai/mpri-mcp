"""Dashboard pages render and their forms work."""

import re

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import select

from mpri_mcp.app import create_app
from mpri_mcp.container import Container
from mpri_mcp.db.models import PriceRecord
from mpri_mcp.services import agent_tasks
from mpri_mcp.services.agent_tasks import PriceResult
from tests.test_admin_auth import csrf, do_setup

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
URL_350 = "https://online.mmvietnam.com/product/nuoc-tinh-khiet-satori-thung-24-chai-350ml.html"


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
@pytest.fixture
def app(settings):
    return create_app(settings, Container(settings))


@pytest.fixture
def client(app):
    with TestClient(app, base_url="http://localhost:8000", follow_redirects=False) as c:
        do_setup(c)
        yield c


def post(client, page_url, target, **data):
    page = client.get(page_url)
    assert page.status_code == 200, page.text
    return client.post(target, data={"csrf_token": csrf(page.text), **data})


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def run_async(client, app, fn):
    """Run a coroutine function with the app's container on the app's event loop."""
    container = app.state.mpri.container

    async def go():
        return await fn(container)

    return client.portal.call(go)


def test_empty_pages_render(client):
    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    for url in [
        "/admin",
        "/admin/supermarkets",
        "/admin/products",
        "/admin/runs",
        "/admin/apps",
        "/admin/account",
    ]:
        resp = client.get(url)
        assert resp.status_code == 200, (url, resp.text[:500])
        assert 'aria-current="page"' in resp.text
    assert "Connect Claude" in client.get("/admin").text
    assert "http://localhost:8000/mcp" in client.get("/admin/apps").text
    assert client.get("/admin/ai").status_code == 404  # AI settings were removed
    assert "AI settings" not in client.get("/admin").text


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def test_supermarket_and_product_workflow(client, app):
    resp = post(
        client,
        "/admin/supermarkets",
        "/admin/supermarkets",
        name="MM Mega Market",
        base_url="online.mmvietnam.com",
        search_url_template="https://online.mmvietnam.com/search.html?query={q}",
    )
    assert (
        resp.status_code == 303 and resp.headers["location"] == "/admin/supermarkets/mm-mega-market"
    )
    detail = client.get("/admin/supermarkets/mm-mega-market")
    assert "Added MM Mega Market" in detail.text and "no schedule" in detail.text
    assert 'value="https://online.mmvietnam.com/search.html?query={q}"' in detail.text
    assert "Crawl settings" not in detail.text and "AI model" not in detail.text

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
    assert "/static/chart.umd.min.js" in page.text and "Re-test" not in page.text

    # Queue crawl: tasks for Claude, nothing fetched by the server.
    resp = post(
        client, "/admin/supermarkets/mm-mega-market", "/admin/supermarkets/mm-mega-market/run"
    )
    assert resp.status_code == 303
    detail = client.get("/admin/supermarkets/mm-mega-market")
    assert "Queued 1 crawl task(s)" in detail.text and "1 task(s) waiting for Claude" in detail.text
    assert "1 queued" in client.get("/admin/supermarkets").text

    # Claude reads the page and sends the price back.
    async def agent(c):
        claimed = await agent_tasks.claim_tasks(c.db, c.settings)
        return await agent_tasks.submit_results(
            c.db,
            c.settings,
            [PriceResult(claimed.tasks[0].task.id, "ok", 65000, 89000, 58998, "65,000 ₫ / Thùng")],
        )

    assert run_async(client, app, agent).accepted == 1
    history = client.get(f"{product_url}/history.json").json()
    assert history["series"][0]["name"] == "MM Mega Market"
    assert history["series"][0]["points"][0]["current"] == 65000
    overview = client.get("/admin")
    assert "success" in overview.text and "Waiting for Claude" in overview.text

    runs = client.get("/admin/runs")
    run_id = re.search(r'href="/admin/runs/(\d+)"', runs.text).group(1)
    detail = client.get(f"/admin/runs/{run_id}")
    assert "Pages read by Claude" in detail.text and URL_350 in detail.text
    assert ">agent<" in detail.text


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def test_search_url_template_is_validated(client):
    post(
        client, "/admin/supermarkets", "/admin/supermarkets", name="Winmart", base_url="winmart.vn"
    )
    page = "/admin/supermarkets/winmart"
    post(
        client,
        page,
        page,
        name="Winmart",
        base_url="winmart.vn",
        search_url_template="https://winmart.vn/search",
    )
    detail = client.get(page).text
    assert "{q}" in detail and 'value="https://winmart.vn/search"' not in detail
    post(
        client,
        page,
        page,
        name="Winmart",
        base_url="winmart.vn",
        search_url_template="https://winmart.vn/search?q={q}",
    )
    assert 'value="https://winmart.vn/search?q={q}"' in client.get(page).text
    post(client, page, page, name="Winmart", base_url="winmart.vn")  # field absent: kept
    assert 'value="https://winmart.vn/search?q={q}"' in client.get(page).text


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def test_review_accept_and_reject(client, app):
    post(
        client,
        "/admin/supermarkets",
        "/admin/supermarkets",
        name="MM Mega Market",
        base_url="online.mmvietnam.com",
    )
    post(
        client,
        "/admin/products",
        "/admin/products",
        name="Satori 350ml",
        **{"url__mm-mega-market": URL_350},
    )

    async def two_crawls(c):
        ids = []
        for price in (100000, 20000, 90000):  # 2nd is -80% (flagged), 3rd is +350% (flagged)
            await agent_tasks.queue_market(c.db, c.settings, "mm-mega-market", "manual")
            claimed = await agent_tasks.claim_tasks(c.db, c.settings)
            await agent_tasks.submit_results(
                c.db, c.settings, [PriceResult(claimed.tasks[0].task.id, "ok", price)]
            )
        async with c.db.session() as s:
            ids = list(
                (
                    await s.scalars(
                        select(PriceRecord.id)
                        .where(PriceRecord.needs_review)
                        .order_by(PriceRecord.id)
                    )
                ).all()
            )
        return ids

    flagged = run_async(client, app, two_crawls)
    assert len(flagged) == 2
    overview = client.get("/admin")
    assert "Prices to review" in overview.text and "20.000 ₫" in overview.text

    resp = post(client, "/admin", f"/admin/prices/{flagged[0]}/accept")
    assert resp.status_code == 303
    resp = post(client, "/admin", f"/admin/prices/{flagged[1]}/reject")
    assert resp.status_code == 303
    assert "Prices to review" not in client.get("/admin").text

    async def remaining(c):
        async with c.db.session() as s:
            return [
                (r.current_price, r.needs_review)
                for r in (await s.scalars(select(PriceRecord).order_by(PriceRecord.id))).all()
            ]

    assert run_async(client, app, remaining) == [(100000, False), (20000, False)]
    assert post(client, "/admin", "/admin/prices/999/accept").status_code == 404


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


def test_pages_require_login(app):
    with TestClient(app, base_url="http://localhost:8000", follow_redirects=False) as c:
        do_setup(c)
        c.cookies.clear()
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        for url in [
            "/admin/supermarkets",
            "/admin/apps",
            "/admin/runs/1",
            "/admin/products/1/history.json",
        ]:
            resp = c.get(url)
            assert resp.status_code == 303 and resp.headers["location"].startswith("/admin/login")
        resp = c.post("/admin/prices/1/reject")
        assert resp.status_code == 303 and resp.headers["location"].startswith("/admin/login")


def test_unknown_ids_render_404(client):
    assert client.get("/admin/supermarkets/nope").status_code == 404
    assert client.get("/admin/products/999").status_code == 404
    assert client.get("/admin/runs/999").status_code == 404
