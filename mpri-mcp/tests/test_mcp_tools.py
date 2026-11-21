"""MCP tools exercised through an in-process MCP client (auth is HTTP-only, tested separately)."""

import pytest
from mcp import Client

from mpri_mcp.container import Container
from mpri_mcp.mcp_server import build_mcp_server

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
URL_500 = "https://online.mmvietnam.com/product/nuoc-tinh-khiet-satori-thung-24-chai-500ml.html"
URL_350 = "https://online.mmvietnam.com/product/nuoc-tinh-khiet-satori-thung-24-chai-350ml.html"


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
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


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
async def test_tool_list_and_annotations(mcp):
    async with Client(mcp) as client:
        tools = {t.name: t for t in (await client.list_tools()).tools}
    assert set(tools) == {
        "get_supermarkets",
        "add_supermarket",
        "update_supermarket",
        "setup_supermarket_cron_task",
        "disable_supermarket_cron_task",
        "trigger_crawl_now",
        "add_product_with_urls",
        "get_product_price_comparison",
        "get_deep_discount_items",
        "get_price_history",
        "get_search_urls",
        "get_pending_crawl_tasks",
        "submit_price_results",
        # TM - #{ticket number} - MPriMCP Price Alerts & Notifications
        "set_price_alert",
        "get_price_alerts",
        "delete_price_alert",
    }
    assert tools["get_supermarkets"].annotations.read_only_hint is True
    assert tools["get_search_urls"].annotations.read_only_hint is True
    assert tools["add_supermarket"].annotations.read_only_hint is False
    schema = tools["add_product_with_urls"].input_schema
    assert set(schema["required"]) == {"name", "urls"}
    # Existing tools keep their required parameters; new ones are optional.
    assert set(tools["add_supermarket"].input_schema["required"]) == {"name", "base_url"}
    assert set(tools["trigger_crawl_now"].input_schema["required"]) == {"supermarket_id"}
    assert set(tools["update_supermarket"].input_schema["required"]) == {"supermarket_id"}
    assert "before lease_expires_at" in tools["get_pending_crawl_tasks"].description.lower()
    assert (
        "Vietnam time"
        in tools["setup_supermarket_cron_task"].input_schema["properties"]["cron_expression"][
            "description"
        ]
    )


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
async def test_user_story_end_to_end(mcp):
    """Setup from the requirements, then an agent crawl and the analytics tools."""
    async with Client(mcp) as client:
        added = await call(
            client,
            "add_supermarket",
            {"name": "MM Mega Market", "base_url": "online.mmvietnam.com"},
        )
        market_id = added["id"]
        assert market_id == "mm-mega-market" and added["search_url_template"] is None

        sched = await call(
            client,
            "setup_supermarket_cron_task",
            {"supermarket_id": market_id, "cron_expression": "0 3 * * *"},
        )
        assert len(sched["next_runs"]) == 3 and "03:00" in sched["next_runs"][0]

        product = await call(
            client,
            "add_product_with_urls",
            {
                "name": "Nước tinh khiết Satori, thùng 24 chai, 350ml",
                "category": "Nước uống",
                "urls": [{"supermarket_id": market_id, "url": URL_350}],
            },
        )
        assert product["created"] and product["urls"][0]["status"] == "added"
        await call(
            client,
            "add_product_with_urls",
            {
                "name": "Nước tinh khiết Satori, thùng 24 chai, 500ml",
                "urls": [{"supermarket_id": market_id, "url": URL_500}],
            },
        )

        listing = await call(client, "get_supermarkets")
        m = listing["supermarkets"][0]
        assert m["schedule"]["enabled"] and m["schedule"]["next_run"]
        assert m["tracked_urls"] == 2 and m["last_run"] is None
        assert (m["pending_tasks"], m["needs_review_count"]) == (0, 0)

        queued = await call(client, "trigger_crawl_now", {"supermarket_id": market_id})
        assert queued["started"] is True and queued["queued_tasks"] == 2

        claimed = await call(client, "get_pending_crawl_tasks", {})
        assert claimed["count"] == 2 and claimed["remaining_pending"] == 0
        by_url = {t["url"]: t for t in claimed["tasks"]}
        assert by_url[URL_350]["product_name"].endswith("350ml")
        assert "last_price" not in by_url[URL_350] and by_url[URL_350]["lease_expires_at"]

        submitted = await call(
            client,
            "submit_price_results",
            {
                "results": [
                    {
                        "task_id": by_url[URL_350]["task_id"],
                        "status": "ok",
                        "price": 65000,
                        "original_price": 89000,
                        "member_price": 58998,
                        "raw_text": "65,000 ₫ / Thùng 89,000 ₫ -27% Giá thành viên MCard 58,998 ₫",
                    },
                    {"task_id": by_url[URL_500]["task_id"], "status": "ok", "price": 109000},
                ]
            },
        )
        assert submitted["accepted"] == 2 and submitted["rejected"] == []
        assert submitted["remaining_pending"] == 0

        listing = await call(client, "get_supermarkets")
        run = listing["supermarkets"][0]["last_run"]
        assert (run["status"], run["trigger"], run["urls_ok"], run["urls_failed"]) == (
            "success",
            "agent",
            2,
            0,
        )

        cmp = await call(client, "get_product_price_comparison", {"product_name": "satori 350"})
        assert cmp["product"]["name"].endswith("350ml")
        row = cmp["cheapest"]
        assert (row["current_price"], row["original_price"], row["member_price"]) == (
            65000,
            89000,
            58998,
        )
        assert row["discount_pct"] == 27.0 and row["source"] == "agent"
        assert "needs_review" not in row

        deals = await call(client, "get_deep_discount_items", {"min_discount_percentage": 20})
        assert deals["count"] == 1 and deals["items"][0]["discount_pct"] == 27.0

        history = await call(client, "get_price_history", {"product_name": "satori 500ml"})
        stats = history["series"][0]["stats"]
        assert stats["latest_price"] == 109000 and stats["latest_vs_min_pct"] == 0.0

        paused = await call(client, "disable_supermarket_cron_task", {"supermarket_id": market_id})
        assert paused["enabled"] is False
        listing = await call(client, "get_supermarkets")
        assert listing["supermarkets"][0]["schedule"]["next_run"] is None


async def test_errors_are_readable(mcp):
    async with Client(mcp) as client:
        bad = await client.call_tool(
            "setup_supermarket_cron_task",
            {"supermarket_id": "nope", "cron_expression": "0 3 * * *"},
        )
        assert bad.is_error and "not found" in bad.content[0].text

        await call(client, "add_supermarket", {"name": "Winmart", "base_url": "winmart.vn"})
        bad = await client.call_tool(
            "setup_supermarket_cron_task",
            {"supermarket_id": "winmart", "cron_expression": "every day"},
        )
        assert bad.is_error and "Invalid cron expression" in bad.content[0].text

        bad = await client.call_tool("get_product_price_comparison", {"product_name": "bia"})
        assert bad.is_error and "No tracked product" in bad.content[0].text

        bad = await client.call_tool("trigger_crawl_now", {"supermarket_id": "winmart"})
        assert bad.is_error and "no active product URLs" in bad.content[0].text

        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        bad = await client.call_tool("update_supermarket", {"supermarket_id": "winmart"})
        assert bad.is_error and "Nothing to update" in bad.content[0].text
        bad = await client.call_tool("get_pending_crawl_tasks", {"limit": 51})
        assert bad.is_error and "between 1 and 50" in bad.content[0].text
        bad = await client.call_tool("get_search_urls", {"keyword": "x", "supermarket_ids": ["?"]})
        assert bad.is_error and "Unknown supermarket" in bad.content[0].text
