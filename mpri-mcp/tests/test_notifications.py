"""Price alerts and notifications (Telegram / Discord are faked with httpx2.MockTransport)."""

# TM - #{ticket number} - MPriMCP Price Alerts & Notifications

import json

import httpx2
import pytest
from fastapi.testclient import TestClient
from mcp import Client
from sqlalchemy import select

from mpri_mcp.app import create_app
from mpri_mcp.container import Container
from mpri_mcp.db.models import AppSetting, NotificationLog
from mpri_mcp.mcp_server import build_mcp_server
from mpri_mcp.services.notifications import (
    DISCORD_WEBHOOK,
    TELEGRAM_TOKEN,
    render_discord,
    render_telegram,
    split_messages,
    update_notify_settings,
)
from mpri_mcp.services.notify_messages import Seg
from tests.test_admin_auth import csrf, do_setup

TOKEN = "123456789:" + "A" * 35
WEBHOOK = "https://discord.com/api/webhooks/1/secret-part"


class FakeChannels:
    """Answers like Telegram's Bot API and a Discord webhook; records what was sent."""

    def __init__(self) -> None:
        self.sent: list[tuple[str, dict]] = []
        self.telegram_status = 200
        self.updates: list[dict] = []

    def handler(self, request: httpx2.Request) -> httpx2.Response:
        url = str(request.url)
        if url.endswith("/getUpdates"):
            return httpx2.Response(200, json={"ok": True, "result": self.updates})
        body = json.loads(request.content or b"{}")
        if "api.telegram.org" in url:
            self.sent.append(("telegram", body))
            if self.telegram_status != 200:
                return httpx2.Response(self.telegram_status, json={"ok": False, "description": "x"})
            return httpx2.Response(200, json={"ok": True})
        self.sent.append(("discord", body))
        return httpx2.Response(204)

    def texts(self, channel: str) -> list[str]:
        return [
            b["text"] if channel == "telegram" else b["content"]
            for c, b in self.sent
            if c == channel
        ]


@pytest.fixture
def fake() -> FakeChannels:
    return FakeChannels()


def wire(container: Container, fake: FakeChannels) -> None:
    container.notifier.client_factory = lambda: httpx2.AsyncClient(
        transport=httpx2.MockTransport(fake.handler)
    )
    container.notifier.backoff = (0, 0, 0)


@pytest.fixture
async def container(settings, fake):
    c = Container(settings)
    wire(c, fake)
    await c.start()
    yield c
    await c.stop()


async def enable_channels(c: Container) -> None:
    c.notifier.secrets.set(TELEGRAM_TOKEN, TOKEN)
    c.notifier.secrets.set(DISCORD_WEBHOOK, WEBHOOK)
    await update_notify_settings(
        c.db, telegram_enabled=True, telegram_chat_id="42", discord_enabled=True
    )


async def call(client: Client, name: str, args: dict | None = None) -> dict:
    result = await client.call_tool(name, args or {})
    assert not result.is_error, result.content
    return result.structured_content


async def setup_products(client: Client) -> None:
    await call(
        client, "add_supermarket", {"name": "MM Mega Market", "base_url": "online.mmvietnam.com"}
    )
    await call(client, "add_supermarket", {"name": "Coop Mart", "base_url": "cooponline.vn"})
    await call(
        client,
        "add_product_with_urls",
        {
            "name": "Nước Satori 500 ml",
            "urls": [
                {"supermarket_id": "mm-mega-market", "url": "https://online.mmvietnam.com/satori"},
                {"supermarket_id": "coop-mart", "url": "https://cooponline.vn/satori"},
            ],
        },
    )
    await call(
        client,
        "add_product_with_urls",
        {
            "name": "Nước giặt Surf 3.1kg",
            "urls": [{"supermarket_id": "coop-mart", "url": "https://cooponline.vn/surf"}],
        },
    )


async def crawl(client: Client, prices: dict[str, int]) -> dict:
    """One full round: queue both supermarkets, read every task, submit the given prices
    (keyed by URL suffix; a missing key is sent as not_found)."""
    for market in ("mm-mega-market", "coop-mart"):
        await call(client, "trigger_crawl_now", {"supermarket_id": market})
    tasks = (await call(client, "get_pending_crawl_tasks", {"limit": 50}))["tasks"]
    results = []
    for t in tasks:
        key = t["url"].split("//", 1)[1]
        if key in prices:
            results.append({"task_id": t["task_id"], "status": "ok", "price": prices[key]})
        else:
            results.append({"task_id": t["task_id"], "status": "not_found"})
    return await call(client, "submit_price_results", {"results": results})


# ---------------------------------------------------------------------- rendering


def test_rendering_escapes_and_links():
    line = [
        Seg("<Sữa> *TH*_ "),
        Seg("65.000 ₫", bold=True),
        Seg("Xem", url="https://x.vn/a?b=1&c=2"),
    ]
    assert render_telegram(line) == (
        '&lt;Sữa&gt; *TH*_ <b>65.000 ₫</b><a href="https://x.vn/a?b=1&amp;c=2">Xem</a>'
    )
    assert render_discord(line) == r"<Sữa\> \*TH\*\_ **65.000 ₫**[Xem](<https://x.vn/a?b=1&c=2>)"
    lines = [[Seg("x" * 30)] for _ in range(10)]
    parts = split_messages(lines, render_discord, 100)
    assert all(len(p) <= 100 for p in parts) and sum(p.count("x" * 30) for p in parts) == 10


def test_secrets_live_in_files_and_env_wins(settings, monkeypatch):
    c = Container(settings)
    c.notifier.secrets.set(TELEGRAM_TOKEN, TOKEN)
    path = settings.data_dir / "secrets" / TELEGRAM_TOKEN
    assert path.read_text(encoding="utf-8") == TOKEN
    assert c.notifier.secrets.hint(TELEGRAM_TOKEN) == "…AAAA"
    assert c.notifier.secrets.source(TELEGRAM_TOKEN) == "dashboard"
    from mpri_mcp.config import Settings

    env_settings = Settings(**{**settings.model_dump(), "telegram_bot_token": "999:" + "B" * 35})
    env = Container(env_settings)
    assert env.notifier.secrets.get(TELEGRAM_TOKEN) == "999:" + "B" * 35
    assert env.notifier.secrets.source(TELEGRAM_TOKEN) == "environment"


# ---------------------------------------------------------------------- alerts + summary


async def test_alert_fires_once_below_target_and_rearms(container, fake):
    await enable_channels(container)
    mcp = build_mcp_server(container, container.oauth)
    async with Client(mcp) as client:
        await setup_products(client)
        alert = await call(
            client, "set_price_alert", {"product_name": "satori 500", "target_price": 70000}
        )
        assert alert["product_name"] == "Nước Satori 500 ml" and alert["created"] is True
        assert alert["below_target_now"] is False

        r = await crawl(
            client,
            {
                "online.mmvietnam.com/satori": 80000,
                "cooponline.vn/satori": 82000,
                "cooponline.vn/surf": 102900,
            },
        )
        await container.notifier.drain()
        assert "alerts_triggered" not in r and r["summary_sent_to"] == ["telegram", "discord"]
        summary = fake.texts("telegram")[-1]
        assert "Giá rẻ nhất" in summary and "Nước giặt Surf 3.1kg" in summary
        assert "<b>102.900 ₫</b> tại Coop Mart" in summary
        assert "Mốc giá" in summary and "chưa đạt" in summary
        assert "Satori 500 ml</a> —" not in summary  # alerted products only in the alert part

        fake.sent.clear()
        r = await crawl(
            client,
            {
                "online.mmvietnam.com/satori": 65000,
                "cooponline.vn/satori": 69000,
                "cooponline.vn/surf": 102900,
            },
        )
        await container.notifier.drain()
        hits = r["alerts_triggered"]
        assert hits[0]["product_name"] == "Nước Satori 500 ml"
        assert [p["price"] for p in hits[0]["prices"]] == [65000, 69000]
        alert_msg = next(t for t in fake.texts("telegram") if "Giá dưới mốc" in t)
        assert "<b>65.000 ₫</b> tại MM Mega Market" in alert_msg and "mốc 70.000 ₫" in alert_msg
        assert any("Giá dưới mốc" in t for t in fake.texts("discord"))
        assert "✅ đạt" in fake.texts("telegram")[-1]

        r = await crawl(
            client,
            {
                "online.mmvietnam.com/satori": 65000,
                "cooponline.vn/satori": 69000,
                "cooponline.vn/surf": 102900,
            },
        )
        assert "alerts_triggered" not in r  # same prices: not announced twice

        r = await crawl(
            client,
            {
                "online.mmvietnam.com/satori": 60000,
                "cooponline.vn/satori": 75000,
                "cooponline.vn/surf": 102900,
            },
        )
        assert [p["price"] for p in r["alerts_triggered"][0]["prices"]] == [60000]  # lower again

        r = await crawl(
            client,
            {
                "online.mmvietnam.com/satori": 60000,
                "cooponline.vn/satori": 68000,
                "cooponline.vn/surf": 102900,
            },
        )
        assert [p["supermarket_id"] for p in r["alerts_triggered"][0]["prices"]] == ["coop-mart"]

        listed = await call(client, "get_price_alerts")
        assert listed["alerts"][0]["below_target_now"] is True
        assert listed["alerts"][0]["cheapest"]["current_price"] == 60000
        deleted = await call(client, "delete_price_alert", {"product_name": "satori"})
        assert deleted["product_name"] == "Nước Satori 500 ml"
        assert (await call(client, "get_price_alerts"))["count"] == 0

        bad = await client.call_tool(
            "set_price_alert", {"product_name": "satori", "target_price": 0}
        )
        assert bad.is_error
        bad = await client.call_tool(
            "set_price_alert",
            {"product_name": "surf", "target_price": 90000, "supermarket_ids": ["mm-mega-market"]},
        )
        assert bad.is_error and "not tracked at" in bad.content[0].text


async def test_summary_waits_for_the_whole_round_and_can_be_switched_off(container, fake):
    await enable_channels(container)
    mcp = build_mcp_server(container, container.oauth)
    async with Client(mcp) as client:
        await setup_products(client)
        await call(client, "trigger_crawl_now", {"supermarket_id": "coop-mart"})
        tasks = (await call(client, "get_pending_crawl_tasks", {}))["tasks"]
        first = await call(
            client,
            "submit_price_results",
            {"results": [{"task_id": tasks[0]["task_id"], "status": "ok", "price": 80000}]},
        )
        assert "summary_sent_to" not in first  # one task still claimed
        last = await call(
            client,
            "submit_price_results",
            {"results": [{"task_id": tasks[1]["task_id"], "status": "error", "note": "timeout"}]},
        )
        assert "summary_sent_to" not in last  # re-queued: the round is not over

        await update_notify_settings(container.db, summary_enabled=False)
        r = await crawl(client, {"cooponline.vn/satori": 80000, "cooponline.vn/surf": 100000})
        await container.notifier.drain()
        assert "summary_sent_to" not in r
        assert not any("Giá rẻ nhất" in t for t in fake.texts("telegram"))

        await update_notify_settings(container.db, summary_enabled=True)
        r = await crawl(client, {"cooponline.vn/satori": 80000})  # surf + MM satori: not found
        await container.notifier.drain()
        summary = fake.texts("discord")[-1]
        assert "Không đọc được lượt này" in summary
        assert "Nước giặt Surf 3.1kg tại Coop Mart" in summary


async def test_without_channels_claude_is_told(container, fake):
    mcp = build_mcp_server(container, container.oauth)
    async with Client(mcp) as client:
        await setup_products(client)
        r = await crawl(client, {"cooponline.vn/satori": 80000})
    assert "No Telegram/Discord channel" in r["notifications"]
    assert fake.sent == []


async def test_failures_are_retried_logged_and_never_leak_the_token(container, fake):
    await enable_channels(container)
    await update_notify_settings(container.db, discord_enabled=False)
    fake.telegram_status = 500
    await container.notifier.notify("test", [[Seg("hello")]])
    await container.notifier.drain()
    assert len(fake.texts("telegram")) == 4  # first try + 3 retries
    fake.sent.clear()
    fake.telegram_status = 400
    await container.notifier.notify("test", [[Seg("hello")]])
    await container.notifier.drain()
    assert len(fake.texts("telegram")) == 1  # a client error is not retried
    async with container.db.session() as s:
        logs = (await s.scalars(select(NotificationLog).order_by(NotificationLog.id))).all()
        settings_row = await s.get(AppSetting, "notifications")
    assert [entry.status for entry in logs] == ["failed", "failed"]
    assert all(TOKEN not in (entry.error or "") for entry in logs)
    assert TOKEN not in json.dumps(settings_row.value)  # the token is never in the database


# ---------------------------------------------------------------------- dashboard


def test_dashboard_channels_test_detect_and_product_alert(settings, fake):
    c = Container(settings)
    wire(c, fake)
    app = create_app(settings, c)
    with TestClient(app, base_url="http://localhost:8000", follow_redirects=False) as client:
        do_setup(client)

        def post(target: str, page: str = "/admin/notifications", **data):
            token = csrf(client.get(page).text)
            return client.post(target, data={"csrf_token": token, **data})

        page = client.get("/admin/notifications")
        assert page.status_code == 200 and "no channel active" in page.text

        post("/admin/notifications/telegram", bot_token="not-a-token", enabled="on")
        assert "does not look like a Telegram bot token" in client.get("/admin/notifications").text
        post("/admin/notifications/telegram", bot_token=TOKEN, enabled="on")
        fake.updates = [{"message": {"chat": {"id": 42, "first_name": "Tan"}}}]
        post("/admin/notifications/telegram/detect")
        page = client.get("/admin/notifications").text
        assert "Chat ID set to 42 (Tan)" in page and TOKEN not in page and "…AAAA" in page

        post("/admin/notifications/discord", webhook_url="https://example.com/hook", enabled="on")
        assert (
            "starts with https://discord.com/api/webhooks/"
            in client.get("/admin/notifications").text
        )
        post("/admin/notifications/discord", webhook_url=WEBHOOK, enabled="on")
        page = client.get("/admin/notifications").text
        assert "sending to telegram + discord" in page and WEBHOOK not in page

        post("/admin/notifications/test/telegram")
        post("/admin/notifications/test/discord")
        page = client.get("/admin/notifications").text
        assert "Test message sent to Discord." in page
        assert [ch for ch, _ in fake.sent] == ["telegram", "discord"]
        assert fake.sent[0][1]["chat_id"] == "42"

        post(
            "/admin/supermarkets", "/admin/supermarkets", name="Coop Mart", base_url="cooponline.vn"
        )
        resp = post(
            "/admin/products",
            "/admin/products",
            name="Nước Satori 500 ml",
            **{"url__coop-mart": "https://cooponline.vn/satori"},
        )
        product = resp.headers["location"]
        post(f"{product}/alert", product, target_price="70.000")
        detail = client.get(product).text
        assert "Alert set: below 70.000 ₫." in detail and 'value="70000"' in detail
        alert_id = client.get("/admin/notifications").text.split("/admin/alerts/")[1].split("/")[0]
        post(f"/admin/alerts/{alert_id}/delete", product, next=f"{product}#alert")
        assert "Price alert removed." in client.get(product).text
