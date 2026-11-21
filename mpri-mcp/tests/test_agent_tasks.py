"""Agent crawl: crawl tasks claimed by Claude, prices submitted back (BR acceptance criteria)."""

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search

import asyncio
from datetime import timedelta

import pytest
from mcp import Client
from sqlalchemy import select, update

from mpri_mcp.container import Container
from mpri_mcp.db.models import CrawlRun, CrawlTask, ProductUrl
from mpri_mcp.mcp_server import build_mcp_server
from mpri_mcp.utils.time import utcnow


@pytest.fixture
async def container(settings):
    c = Container(settings)
    await c.start()
    yield c
    await c.stop()


@pytest.fixture
def mcp(container):
    return build_mcp_server(container, container.oauth)


async def call(client: Client, name: str, args: dict | None = None) -> dict:
    result = await client.call_tool(name, args or {})
    assert not result.is_error, result.content
    return result.structured_content


async def market_with_urls(client: Client, name: str, domain: str, count: int) -> str:
    market = await call(client, "add_supermarket", {"name": name, "base_url": domain})
    for n in range(1, count + 1):
        await call(
            client,
            "add_product_with_urls",
            {
                "name": f"Sản phẩm {n}",
                "category": "Test",
                "urls": [{"supermarket_id": market["id"], "url": f"https://{domain}/p{n}"}],
            },
        )
    return market["id"]


async def market_row(client: Client, market_id: str) -> dict:
    listing = await call(client, "get_supermarkets")
    return next(m for m in listing["supermarkets"] if m["id"] == market_id)


async def claim_all(client: Client, market_id: str | None = None) -> list[dict]:
    args = {"limit": 50, **({"supermarket_id": market_id} if market_id else {})}
    return (await call(client, "get_pending_crawl_tasks", args))["tasks"]


async def submit(client: Client, *results: dict) -> dict:
    return await call(client, "submit_price_results", {"results": list(results)})


async def test_queue_skips_open_tasks_and_claims_are_atomic(mcp, container):
    """AC-3, AC-4, AC-5."""
    async with Client(mcp) as client:
        lotte = await market_with_urls(client, "Lotte Mart", "lottemart.vn", 6)

        queued = await call(client, "trigger_crawl_now", {"supermarket_id": lotte})
        assert (queued["queued_tasks"], queued["pending_tasks"]) == (6, 6)
        row = await market_row(client, lotte)
        assert row["pending_tasks"] == 6
        assert (row["last_run"]["status"], row["last_run"]["trigger"]) == ("queued", "manual")

        again = await call(client, "trigger_crawl_now", {"supermarket_id": lotte})
        assert (again["queued_tasks"], again["already_open"], again["started"]) == (0, 6, False)
        async with container.db.session() as s:
            assert len((await s.scalars(select(CrawlTask))).all()) == 6

    async def claim3() -> list[str]:
        async with Client(mcp) as c:
            r = await call(c, "get_pending_crawl_tasks", {"limit": 3})
            return [t["task_id"] for t in r["tasks"]]

    a, b = await asyncio.gather(claim3(), claim3())
    assert len(a) == len(b) == 3 and not set(a) & set(b)
    async with Client(mcp) as client:
        assert (await market_row(client, lotte))["pending_tasks"] == 6  # pending + claimed
        assert (await call(client, "get_pending_crawl_tasks", {}))["count"] == 0


async def test_cron_trigger_queues_like_manual(mcp, container):
    async with Client(mcp) as client:
        coop = await market_with_urls(client, "Coop", "cooponline.vn", 2)
    await container._queue_market(coop, "cron")  # what the scheduler job calls
    async with container.db.session() as s:
        run = (await s.scalars(select(CrawlRun))).one()
    assert (run.trigger, run.status, run.urls_total) == ("cron", "queued", 2)


async def test_submit_validation_flagging_and_run_result(mcp, container):
    """AC-6, AC-7, AC-8, AC-11."""
    async with Client(mcp) as client:
        coop = await market_with_urls(client, "Coop", "cooponline.vn", 4)
        await call(client, "trigger_crawl_now", {"supermarket_id": coop})
        t = {x["url"].rsplit("/", 1)[1]: x["task_id"] for x in await claim_all(client)}

        bad = await submit(
            client,
            {"task_id": t["p1"], "status": "ok", "price": 0},
            {"task_id": t["p2"], "status": "ok", "price": 50000, "original_price": 40000},
            {"task_id": "nope", "status": "ok", "price": 1000},
            {"task_id": t["p3"], "status": "ok", "price": 1000, "raw_text": "x" * 501},
        )
        reasons = {r["task_id"]: r["reason"] for r in bad["rejected"]}
        assert bad["accepted"] == 0 and set(reasons) == {t["p1"], t["p2"], "nope", t["p3"]}
        assert "greater than 0" in reasons[t["p1"]]
        assert "lower than price" in reasons[t["p2"]]
        assert "Unknown task_id" in reasons["nope"] and "500" in reasons[t["p3"]]

        ok = await submit(
            client,
            {
                "task_id": t["p1"],
                "status": "ok",
                "price": 80000,
                "original_price": 118000,
                "raw_text": "80,000 ₫ 118,000 ₫",
            },
            {"task_id": t["p2"], "status": "ok", "price": 50000, "member_price": 60000},
            {"task_id": t["p3"], "status": "out_of_stock", "note": "Hết hàng"},
            {"task_id": t["p4"], "status": "not_found"},
        )
        assert ok["accepted"] == 4 and ok["rejected"] == [] and ok["remaining_pending"] == 0
        assert "member_price 60000 is not below price" in ok["warnings"][0]

        p1 = await call(client, "get_product_price_comparison", {"product_name": "Sản phẩm 1"})
        assert (p1["prices"][0]["current_price"], p1["prices"][0]["source"]) == (80000, "agent")
        p3 = await call(client, "get_product_price_comparison", {"product_name": "Sản phẩm 3"})
        assert p3["prices"][0]["in_stock"] is False and p3["prices"][0]["no_data_yet"]

        row = await market_row(client, coop)
        run = row["last_run"]
        assert (run["status"], run["trigger"], run["urls_ok"], run["urls_failed"]) == (
            "partial",
            "agent",
            3,
            1,
        )
        assert row["failing_urls"] == 1 and row["pending_tasks"] == 0

        # A second crawl: p1 drops from 80,000 to 30,000 (-62.5%) -> saved but flagged.
        await call(client, "trigger_crawl_now", {"supermarket_id": coop})
        t = {x["url"].rsplit("/", 1)[1]: x["task_id"] for x in await claim_all(client)}
        flagged = await submit(
            client, {"task_id": t["p1"], "status": "ok", "price": 30000, "original_price": 118000}
        )
        assert flagged["accepted"] == 1
        assert flagged["flagged"][0]["last_price"] == 80000
        assert flagged["flagged"][0]["change_pct"] == -62.5

        p1 = await call(client, "get_product_price_comparison", {"product_name": "Sản phẩm 1"})
        assert p1["prices"][0]["current_price"] == 30000 and p1["prices"][0]["needs_review"]
        deals = await call(client, "get_deep_discount_items", {"min_discount_percentage": 10})
        assert deals["count"] == 0  # the flagged 75% "discount" is left out
        hist = await call(client, "get_price_history", {"product_name": "Sản phẩm 1"})
        assert hist["series"][0]["points"][-1]["needs_review"] is True
        assert (await market_row(client, coop))["needs_review_count"] == 1

        done_again = await submit(client, {"task_id": t["p1"], "status": "ok", "price": 1})
        assert "already done" in done_again["rejected"][0]["reason"]


async def test_expired_lease_returns_task_to_the_queue(mcp, container):
    """AC-9."""
    async with Client(mcp) as client:
        bhx = await market_with_urls(client, "Bách Hóa Xanh", "bachhoaxanh.com", 2)
        await call(client, "trigger_crawl_now", {"supermarket_id": bhx})
        first = await claim_all(client)
        assert len(first) == 2
        assert (await call(client, "get_pending_crawl_tasks", {}))["count"] == 0

        # The lease of every claimed task runs out.
        async with container.db.session() as s:
            await s.execute(
                update(CrawlTask).values(lease_expires_at=utcnow() - timedelta(minutes=1))
            )
        assert (await market_row(client, bhx))["pending_tasks"] == 2
        async with container.db.session() as s:
            assert {t.status for t in (await s.scalars(select(CrawlTask))).all()} == {"pending"}

        late = await submit(client, {"task_id": first[0]["task_id"], "status": "ok", "price": 9000})
        assert late["accepted"] == 1  # lease expired but not claimed again yet: still accepted
        again = await claim_all(client)
        assert [x["task_id"] for x in again] == [first[1]["task_id"]]


async def test_error_is_retried_until_the_third_attempt(mcp, container):
    """AC-10."""
    async with Client(mcp) as client:
        lotte = await market_with_urls(client, "Lotte Mart", "lottemart.vn", 1)
        await call(client, "trigger_crawl_now", {"supermarket_id": lotte})
        task_id = (await claim_all(client))[0]["task_id"]
        for _ in range(2):
            r = await submit(client, {"task_id": task_id, "status": "error", "note": "timeout"})
            assert r["requeued"] == 1
            assert [x["task_id"] for x in await claim_all(client)] == [task_id]
        r = await submit(client, {"task_id": task_id, "status": "error", "note": "timeout"})
        assert "requeued" not in r and r["accepted"] == 1
        async with container.db.session() as s:
            task = await s.get(CrawlTask, task_id)
            assert (task.status, task.attempts, task.last_error) == ("failed", 3, "timeout")
            url = await s.get(ProductUrl, task.product_url_id)
            assert url.consecutive_failures == 1 and url.last_error == "timeout"
        assert (await call(client, "get_pending_crawl_tasks", {}))["count"] == 0
        row = await market_row(client, lotte)
        run = row["last_run"]
        assert (run["status"], run["trigger"], run["urls_ok"], run["urls_failed"]) == (
            "failed",
            "agent",
            0,
            1,
        )
        assert row["failing_urls"] == 1


async def test_pending_tasks_expire_after_48_hours(mcp, container):
    async with Client(mcp) as client:
        lotte = await market_with_urls(client, "Lotte Mart", "lottemart.vn", 2)
        await call(client, "trigger_crawl_now", {"supermarket_id": lotte})
        async with container.db.session() as s:
            await s.execute(update(CrawlTask).values(created_at=utcnow() - timedelta(hours=49)))
        row = await market_row(client, lotte)
        assert row["pending_tasks"] == 0
        assert (row["last_run"]["status"], row["last_run"]["urls_failed"]) == ("failed", 2)
        async with container.db.session() as s:
            assert {t.status for t in (await s.scalars(select(CrawlTask))).all()} == {"expired"}
        # Expired tasks no longer block a new crawl.
        assert (await call(client, "trigger_crawl_now", {"supermarket_id": lotte}))[
            "queued_tasks"
        ] == 2


async def test_update_supermarket_and_search_urls(mcp):
    """AC-12, AC-13."""
    async with Client(mcp) as client:
        await call(client, "add_supermarket", {"name": "Coop Mart", "base_url": "cooponline.vn"})
        await call(
            client,
            "add_supermarket",
            {
                "name": "MM Mega Market",
                "base_url": "online.mmvietnam.com",
                "search_url_template": "https://online.mmvietnam.com/search.html?query={q}",
            },
        )
        bad = await client.call_tool(
            "update_supermarket",
            {"supermarket_id": "coop-mart", "search_url_template": "https://x.vn/search"},
        )
        assert bad.is_error and "{q}" in bad.content[0].text

        upd = await call(
            client,
            "update_supermarket",
            {
                "supermarket_id": "coop-mart",
                "search_url_template": "https://cooponline.vn/search?router=productListing&query={q}",
            },
        )
        assert upd["updated"] == ["search_url_template"] and upd["name"] == "Coop Mart"

        found = await call(client, "get_search_urls", {"keyword": "sữa TH ít đường"})
        urls = {m["supermarket_id"]: m for m in found["supermarkets"]}
        assert urls["coop-mart"]["search_url"].endswith(
            "query=s%E1%BB%AFa%20TH%20%C3%ADt%20%C4%91%C6%B0%E1%BB%9Dng"
        )
        assert "note" not in urls["coop-mart"] or urls["coop-mart"]["note"] is None

        await call(
            client, "update_supermarket", {"supermarket_id": "coop-mart", "search_url_template": ""}
        )
        found = await call(
            client, "get_search_urls", {"keyword": "sữa", "supermarket_ids": ["coop-mart"]}
        )
        only = found["supermarkets"][0]
        assert only["search_url"] is None
        assert only["note"] == "open base_url and use the site's search box"
        assert only["base_url"] == "https://cooponline.vn"
