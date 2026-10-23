"""Shared plumbing for server-rendered pages: templates, sessions, CSRF, flash messages."""

from __future__ import annotations

import hmac
import secrets
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING, Any
from urllib.parse import quote, urlparse
from zoneinfo import ZoneInfo

from babel.numbers import format_decimal
from fastapi import HTTPException, Request
from fastapi.templating import Jinja2Templates
from starlette.responses import Response

from mpri_mcp.auth.admin import admin_count, get_admin
from mpri_mcp.db.models import AdminUser
from mpri_mcp.i18n import (
    LANGUAGES,
    _,
    format_relative,
    format_weekday_datetime,
    get_language,
    gettext,
    ngettext,
    translate_stored,
)

if TYPE_CHECKING:
    from mpri_mcp.app import AppState

TEMPLATES_DIR = Path(__file__).parent / "templates"
STATIC_DIR = Path(__file__).parent / "static"

LANGUAGE_COOKIE = "mpri_lang"

templates = Jinja2Templates(directory=TEMPLATES_DIR)
templates.env.add_extension("jinja2.ext.i18n")
# newstyle: _("Hi %(name)s", name=x) formats and escapes the variables.
templates.env.install_gettext_callables(gettext, ngettext, newstyle=True)  # type: ignore[attr-defined]


class SetupRequired(Exception):
    """No admin account yet: send the browser to /admin/setup."""


class LoginRequired(Exception):
    def __init__(self, next_url: str) -> None:
        self.next_url = next_url


def state(request: Request) -> AppState:
    return request.app.state.mpri


# --------------------------------------------------------------------------- session
def csrf_token(request: Request) -> str:
    token = request.session.get("csrf")
    if not token:
        token = secrets.token_urlsafe(32)
        request.session["csrf"] = token
    return token


async def verify_csrf(request: Request) -> None:
    form = await request.form()
    sent = form.get("csrf_token") or request.headers.get("x-csrf-token") or ""
    expected = request.session.get("csrf") or ""
    if not (isinstance(sent, str) and sent and hmac.compare_digest(sent, expected)):
        raise HTTPException(status_code=403, detail=_("Form expired or invalid. Reload the page."))


def login_session(request: Request, user: AdminUser) -> None:
    request.session.clear()  # new session id-equivalent: drop anything pre-login
    request.session.update(
        {"uid": user.id, "sv": user.session_version, "csrf": secrets.token_urlsafe(32)}
    )


def logout_session(request: Request) -> None:
    request.session.clear()


def flash(request: Request, message: str, category: str = "info") -> None:
    request.session["flash"] = [*request.session.get("flash", []), [category, message]]


def pop_flashes(request: Request) -> list[list[str]]:
    return request.session.pop("flash", [])


async def admin_exists(request: Request) -> bool:
    st = state(request)
    if not st.admin_exists:
        st.admin_exists = await admin_count(st.db) > 0
    return st.admin_exists


async def current_admin(request: Request) -> AdminUser | None:
    uid = request.session.get("uid")
    if uid is None:
        return None
    user = await get_admin(state(request).db, uid)
    if user is None or user.session_version != request.session.get("sv"):
        logout_session(request)
        return None
    return user


async def require_admin(request: Request) -> AdminUser:
    """FastAPI dependency for every dashboard page."""
    if not await admin_exists(request):
        raise SetupRequired()
    user = await current_admin(request)
    if user is None:
        target = request.url.path + (f"?{request.url.query}" if request.url.query else "")
        raise LoginRequired(target)
    request.state.admin = user
    return user


def safe_next(next_url: str | None, default: str = "/admin") -> str:
    """Only allow same-site relative redirects after login."""
    if not next_url:
        return default
    parsed = urlparse(next_url)
    if parsed.scheme or parsed.netloc or not next_url.startswith("/") or next_url.startswith("//"):
        return default
    return next_url


def login_redirect_url(next_url: str) -> str:
    return f"/admin/login?next={quote(next_url, safe='')}"


def client_ip(request: Request) -> str:
    return request.client.host if request.client else "unknown"


def render(request: Request, name: str, status_code: int = 200, **context: Any) -> Response:
    return templates.TemplateResponse(
        request,
        name,
        {"admin": getattr(request.state, "admin", None), **context},
        status_code=status_code,
    )


# --------------------------------------------------------------------------- filters
@dataclass
class _Display:
    tz: ZoneInfo


_display = _Display(ZoneInfo("Asia/Ho_Chi_Minh"))


def configure_display(timezone: str) -> None:
    _display.tz = ZoneInfo(timezone)


def fmt_vnd(value: int | float | None) -> str:
    if value is None:
        return "—"
    return f"{int(value):,}".replace(",", ".") + " ₫"


def fmt_dt(value: datetime | None, fmt: str = "%Y-%m-%d %H:%M") -> str:
    if value is None:
        return "—"
    return value.astimezone(_display.tz).strftime(fmt)


def fmt_dt_weekday(value: datetime | None) -> str:
    """'Sat 24/10/2026 03:00' / 'Thứ Bảy, 24/10/2026 03:00'."""
    return format_weekday_datetime(value, _display.tz.key) or "—"


def fmt_ago(value: datetime | None) -> str:
    return format_relative(value)


def fmt_pct(value: float | None) -> str:
    return "—" if value is None else format_decimal(value, locale=get_language()) + "%"


def fmt_usd(value: float | None) -> str:
    if value is None:
        return "—"
    if value == 0:
        return "$0"
    if value < 0.01:
        return "<$" + format_decimal(0.01, format="0.00", locale=get_language())
    return "$" + format_decimal(value, format="#,##0.00", locale=get_language())


def fmt_usd4(value: float | None) -> str:
    if value is None:
        return "—"
    return "$" + format_decimal(value, format="#,##0.0000", locale=get_language())


def fmt_int(value: int | None) -> str:
    return "—" if value is None else format_decimal(value, locale=get_language())


templates.env.globals.update(
    csrf_token=csrf_token,
    pop_flashes=pop_flashes,
    current_language=get_language,
    LANGUAGES=LANGUAGES,
)
templates.env.filters.update(
    vnd=fmt_vnd,
    dt=fmt_dt,
    dtw=fmt_dt_weekday,
    ago=fmt_ago,
    pct=fmt_pct,
    num=fmt_int,
    usd=fmt_usd,
    usd4=fmt_usd4,
    tr=translate_stored,
)


async def form_data(request: Request) -> dict[str, str]:
    """String fields of the submitted form (file uploads are ignored)."""
    form = await request.form()
    return {k: v for k, v in form.items() if isinstance(v, str)}
