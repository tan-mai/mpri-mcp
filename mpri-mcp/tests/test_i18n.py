"""Translations: catalog completeness, formatting, per-connection MCP language, dashboard."""

import re
from datetime import datetime, timedelta
from pathlib import Path
from urllib.parse import parse_qs, urlparse
from zoneinfo import ZoneInfo

import pytest
from babel.messages.extract import extract_from_dir
from babel.messages.pofile import read_po
from fastapi.testclient import TestClient

from mpri_mcp.app import create_app
from mpri_mcp.i18n import (
    LOCALES_DIR,
    _,
    format_relative,
    format_weekday_datetime,
    get_language,
    normalize_language,
    translate_stored,
    use_language,
)
from mpri_mcp.services import supermarkets
from mpri_mcp.services.errors import NotFoundError
from mpri_mcp.utils.time import utcnow
from tests.test_admin_auth import csrf, do_setup
from tests.test_oauth import BASE, exchange, initialize, mcp_call, pkce, register

ROOT = Path(__file__).resolve().parents[1]
TZ = "Asia/Ho_Chi_Minh"

# --------------------------------------------------------------------------- catalog


def _source_messages() -> set[str]:
    method_map = [
        ("src/mpri_mcp/**.py", "python"),
        ("src/mpri_mcp/admin/templates/**.html", "jinja2"),
    ]
    options = {
        "src/mpri_mcp/admin/templates/**.html": {
            "extensions": "jinja2.ext.i18n",
            "newstyle_gettext": "true",
        }
    }
    found = set()
    for _filename, _lineno, message, _comments, _context in extract_from_dir(
        str(ROOT), method_map, options
    ):
        if isinstance(message, str) and message:
            found.add(message)
    return found


def _vi_catalog() -> dict[str, str]:
    with (LOCALES_DIR / "vi" / "LC_MESSAGES" / "messages.po").open("rb") as f:
        # Fuzzy entries (auto-guessed by `pybabel update`) are ignored at runtime, so here too.
        return {str(m.id): str(m.string) for m in read_po(f) if m.id and not m.fuzzy}


_PLACEHOLDER = re.compile(r"\{[a-z_]+\}|%\([a-z_]+\)s")
_TAG = re.compile(r"</?[a-z]+[^>]*>")


def test_every_source_string_has_a_vietnamese_translation():
    """Fails when someone adds _("...") without updating messages.po — see README."""
    catalog = _vi_catalog()
    missing = sorted(m for m in _source_messages() if not catalog.get(m))
    assert not missing, "Run the translation workflow in README.md. Missing:\n" + "\n".join(missing)


@pytest.mark.parametrize("msgid", sorted(_vi_catalog()))
def test_translation_keeps_placeholders_and_markup(msgid):
    translated = _vi_catalog()[msgid]
    assert sorted(_PLACEHOLDER.findall(msgid)) == sorted(_PLACEHOLDER.findall(translated))
    assert sorted(_TAG.findall(msgid)) == sorted(_TAG.findall(translated))
    # Templates format every string with %, so a lone % would crash rendering.
    assert "%" not in _PLACEHOLDER.sub("", translated).replace("%%", "")


# --------------------------------------------------------------------------- runtime


def test_gettext_and_fallback():
    assert get_language() == "en"
    assert _("Saved.") == "Saved."
    with use_language("vi"):
        assert _("Saved.") == "Đã lưu."
        assert _("A string nobody translated") == "A string nobody translated"
        with use_language("en"):
            assert _("Saved.") == "Saved."
        assert get_language() == "vi"
    assert get_language() == "en"


@pytest.mark.parametrize(
    ("raw", "code"), [("vi", "vi"), ("VI", "vi"), ("vi-VN", "vi"), ("en_US", "en"), ("fr", None)]
)
def test_normalize_language(raw, code):
    assert normalize_language(raw) == code


def test_dates_and_relative_times():
    when = datetime(2026, 10, 24, 3, 0, tzinfo=ZoneInfo(TZ))
    assert format_weekday_datetime(when, TZ) == "Sat 24/10/2026 03:00"
    with use_language("vi"):
        assert format_weekday_datetime(when, TZ) == "Thứ Bảy, 24/10/2026 03:00"
        assert format_relative(utcnow() - timedelta(hours=2)) == "2 giờ trước"
        assert format_relative(None) == "chưa bao giờ"
        assert format_relative(utcnow()) == "vừa xong"
    assert format_relative(utcnow() - timedelta(hours=2)) == "2 hours ago"


def test_stored_crawl_errors_translate_when_known():
    with use_language("vi"):
        assert translate_stored("Timed out") == "Quá thời gian chờ"
        assert translate_stored("HTTP 404") == "HTTP 404"
        assert translate_stored(None) is None


async def test_service_errors_follow_the_current_language(db):
    with pytest.raises(NotFoundError, match="not found"):
        await supermarkets.get_supermarket(db, "nope")
    with use_language("vi"), pytest.raises(NotFoundError, match="Không tìm thấy siêu thị"):
        await supermarkets.get_supermarket(db, "nope")


# --------------------------------------------------------------------------- MCP


@pytest.fixture
def client(settings):
    with TestClient(create_app(settings), base_url=BASE, follow_redirects=False) as c:
        do_setup(c)
        yield c


def connect(client, language: str | None = None) -> str:
    """OAuth flow through the consent screen, optionally choosing a language there."""
    reg = register(client)
    verifier, challenge = pkce()
    resp = client.get(
        "/authorize",
        params={
            "response_type": "code",
            "client_id": reg["client_id"],
            "redirect_uri": "https://claude.ai/api/mcp/auth_callback",
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "resource": f"{BASE}/mcp",
        },
    )
    consent_url = resp.headers["location"]
    page = client.get(consent_url)
    data = {
        "csrf_token": csrf(page.text),
        "req": parse_qs(urlparse(consent_url).query)["req"][0],
        "decision": "allow",
    }
    if language:
        data["language"] = language
    resp = client.post("/oauth/consent", data=data)
    code = parse_qs(urlparse(resp.headers["location"]).query)["code"][0]
    return exchange(client, reg, code, verifier).json()["access_token"]


def call_tool(client, token: str, name: str, arguments: dict) -> dict:
    assert initialize(client, token).status_code == 200
    resp = mcp_call(client, token, "tools/call", {"name": name, "arguments": arguments})
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]


def test_connections_have_independent_languages(client):
    english = connect(client)
    vietnamese = connect(client)

    result = call_tool(client, vietnamese, "set_response_language", {"language": "vi"})
    assert "tiếng Việt" in result["structuredContent"]["message"]

    vi = call_tool(
        client,
        vietnamese,
        "add_supermarket",
        {"name": "Bách Hóa Xanh", "base_url": "bachhoaxanh.com"},
    )
    assert vi["structuredContent"]["message"].startswith("Đã thêm Bách Hóa Xanh")

    en = call_tool(
        client,
        english,
        "setup_supermarket_cron_task",
        {"supermarket_id": "bach-hoa-xanh", "cron_expression": "0 3 * * *"},
    )
    assert en["structuredContent"]["message"].startswith("Crawl schedule for")
    assert re.match(r"[A-Z][a-z]{2} \d\d/\d\d/\d{4} 03:00", en["structuredContent"]["next_runs"][0])

    vi = call_tool(
        client,
        vietnamese,
        "setup_supermarket_cron_task",
        {"supermarket_id": "bach-hoa-xanh", "cron_expression": "0 3 * * *"},
    )
    assert vi["structuredContent"]["next_runs"][0].startswith(("Thứ", "Chủ Nhật"))

    error = call_tool(client, vietnamese, "get_product_price_comparison", {"product_name": "sữa"})
    assert error["isError"] and "Không có sản phẩm" in error["content"][0]["text"]

    back = call_tool(client, vietnamese, "set_response_language", {"language": "en"})
    assert "English" in back["structuredContent"]["message"]


def test_language_chosen_on_consent_screen(client):
    token = connect(client, language="vi")
    result = call_tool(
        client, token, "add_supermarket", {"name": "Winmart", "base_url": "winmart.vn"}
    )
    assert result["structuredContent"]["message"].startswith("Đã thêm Winmart")


def test_language_survives_token_refresh(client):
    reg = register(client)
    verifier, challenge = pkce()
    resp = client.get(
        "/authorize",
        params={
            "response_type": "code",
            "client_id": reg["client_id"],
            "redirect_uri": "https://claude.ai/api/mcp/auth_callback",
            "code_challenge": challenge,
            "code_challenge_method": "S256",
        },
    )
    consent_url = resp.headers["location"]
    page = client.get(consent_url)
    resp = client.post(
        "/oauth/consent",
        data={
            "csrf_token": csrf(page.text),
            "req": parse_qs(urlparse(consent_url).query)["req"][0],
            "decision": "allow",
            "language": "vi",
        },
    )
    code = parse_qs(urlparse(resp.headers["location"]).query)["code"][0]
    tokens = exchange(client, reg, code, verifier).json()
    refreshed = client.post(
        "/token",
        data={
            "grant_type": "refresh_token",
            "refresh_token": tokens["refresh_token"],
            "client_id": reg["client_id"],
            "client_secret": reg["client_secret"],
        },
    ).json()
    result = call_tool(
        client,
        refreshed["access_token"],
        "add_supermarket",
        {"name": "Go!", "base_url": "go-vietnam.vn"},
    )
    assert result["structuredContent"]["message"].startswith("Đã thêm Go!")


def test_dashboard_can_change_a_connections_language(client):
    token = connect(client)
    page = client.get("/admin/apps")
    family = re.search(r"/admin/connections/([0-9a-f]+)/language", page.text).group(1)
    client.post(
        f"/admin/connections/{family}/language",
        data={"csrf_token": csrf(page.text), "language": "vi"},
    )
    result = call_tool(
        client, token, "add_supermarket", {"name": "Lotte Mart", "base_url": "lottemart.vn"}
    )
    assert result["structuredContent"]["message"].startswith("Đã thêm Lotte Mart")


def test_default_language_setting(settings):
    settings.default_language = "vi"
    with TestClient(create_app(settings), base_url=BASE, follow_redirects=False) as c:
        do_setup(c)
        assert 'lang="vi"' in c.get("/admin").text
        token = connect(c)  # consent pre-selects the default
        result = call_tool(
            c, token, "add_supermarket", {"name": "Co.op", "base_url": "cooponline.vn"}
        )
        assert result["structuredContent"]["message"].startswith("Đã thêm Co.op")


# --------------------------------------------------------------------------- dashboard


def test_dashboard_language_switch(client):
    page = client.get("/admin")
    assert '<html lang="en">' in page.text and "Overview" in page.text
    resp = client.post(
        "/admin/language", data={"csrf_token": csrf(page.text), "lang": "vi", "next": "/admin/apps"}
    )
    assert resp.status_code == 303 and resp.headers["location"] == "/admin/apps"
    assert "mpri_lang=vi" in resp.headers["set-cookie"]
    page = client.get("/admin")
    assert '<html lang="vi">' in page.text
    assert "Tổng quan" in page.text and "Kết nối Claude" in page.text
    for url in [
        "/admin/supermarkets",
        "/admin/products",
        "/admin/runs",
        "/admin/ai",
        "/admin/apps",
        "/admin/account",
    ]:
        resp = client.get(url)
        assert resp.status_code == 200 and '<html lang="vi">' in resp.text, url
    # Unknown values fall back to the default; open redirects are refused.
    resp = client.post(
        "/admin/language",
        data={"csrf_token": csrf(page.text), "lang": "xx", "next": "https://evil"},
    )
    assert resp.headers["location"] == "/admin" and "mpri_lang=en" in resp.headers["set-cookie"]


def test_language_switch_works_before_login(settings):
    with TestClient(create_app(settings), base_url=BASE, follow_redirects=False) as c:
        page = c.get("/admin/setup")
        c.post(
            "/admin/language",
            data={"csrf_token": csrf(page.text), "lang": "vi", "next": "/admin/setup"},
        )
        page = c.get("/admin/setup")
        assert "Tạo tài khoản quản trị" in page.text
        resp = c.post(
            "/admin/setup",
            data={
                "csrf_token": csrf(page.text),
                "setup_token": "x",
                "username": "tan",
                "password": "p",
                "confirm": "p",
            },
        )
        assert "Mã thiết lập không đúng" in resp.text


def test_flash_messages_are_translated(client):
    page = client.get("/admin")
    client.post("/admin/language", data={"csrf_token": csrf(page.text), "lang": "vi", "next": "/"})
    page = client.get("/admin/supermarkets")
    client.post(
        "/admin/supermarkets",
        data={"csrf_token": csrf(page.text), "name": "MM", "base_url": "online.mmvietnam.com"},
    )
    assert "Đã thêm MM (ID mm)" in client.get("/admin/supermarkets/mm").text
