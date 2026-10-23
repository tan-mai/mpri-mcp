"""MCP tools exercised through an in-process MCP client (auth is HTTP-only, tested separately)."""

import asyncio

import pytest
from mcp import Client
from tenacity import wait_none

from mpri_mcp.ai.registry import ProviderRegistry
from mpri_mcp.container import Container
from mpri_mcp.mcp_server import build_mcp_server
from tests.fakes import FakeProvider
from tests.test_crawler import URL_350, URL_500, FakeBrowser, fixture, satori_responder


@pytest.fixture
async def container(settings):
    registry = ProviderRegistry(
        settings,
        providers={"fake": FakeProvider(responder=satori_responder)},
        retry_wait=wait_none(),
    )
    browser = FakeBrowser(
        {URL_500: fixture("mm_satori_500ml")["raw"], URL_350: fixture("mm_satori_350ml")["raw"]}
    )
    c = Container(settings, registry=registry, browser=browser)  # type: ignore[arg-type]
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


async def test_tool_list_and_annotations(mcp):
    async with Client(mcp) as client:
        tools = {t.name: t for t in (await client.list_tools()).tools}
    assert set(tools) == {
        "get_supermarkets",
        "add_supermarket",
        "setup_supermarket_cron_task",
        "disable_supermarket_cron_task",
        "trigger_crawl_now",
        "add_product_with_urls",
        "get_product_price_comparison",
        "get_deep_discount_items",
        "get_price_history",
        "set_response_language",
    }
    assert tools["get_supermarkets"].annotations.read_only_hint is True
    assert tools["add_supermarket"].annotations.read_only_hint is False
    schema = tools["add_product_with_urls"].input_schema
    assert set(schema["required"]) == {"name", "urls"}
    assert (
        "Vietnam time"
        in tools["setup_supermarket_cron_task"].input_schema["properties"]["cron_expression"][
            "description"
        ]
    )


async def test_user_story_end_to_end(mcp, container):
    """The scenario from the requirements, plus a crawl and the analytics tools."""
    async with Client(mcp) as client:
        added = await call(
            client,
            "add_supermarket",
            {"name": "MM Mega Market", "base_url": "online.mmvietnam.com"},
        )
        market_id = added["id"]
        assert market_id == "mm-mega-market"

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

        started = await call(client, "trigger_crawl_now", {"supermarket_id": market_id})
        assert started["started"] is True
        for _ in range(100):
            if not container.scheduler.is_running(market_id):
                break
            await asyncio.sleep(0.02)

        listing = await call(client, "get_supermarkets")
        assert listing["supermarkets"][0]["last_run"]["status"] == "success"

        cmp = await call(client, "get_product_price_comparison", {"product_name": "satori 350"})
        assert cmp["product"]["name"].endswith("350ml")
        row = cmp["cheapest"]
        assert (row["current_price"], row["original_price"], row["member_price"]) == (
            65000,
            89000,
            58998,
        )
        assert row["discount_pct"] == 27.0

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
