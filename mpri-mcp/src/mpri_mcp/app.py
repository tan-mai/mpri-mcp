"""ASGI application factory: FastAPI parent app hosting the dashboard, OAuth and MCP."""

from __future__ import annotations

import logging
import secrets
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from dataclasses import dataclass
from pathlib import Path

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse, RedirectResponse
from fastapi.staticfiles import StaticFiles
from starlette.datastructures import MutableHeaders
from starlette.exceptions import HTTPException
from starlette.middleware.sessions import SessionMiddleware
from starlette.responses import Response
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from mpri_mcp import __version__
from mpri_mcp.admin import routes_auth, routes_pages
from mpri_mcp.admin.web import (
    STATIC_DIR,
    LoginRequired,
    SetupRequired,
    configure_display,
    login_redirect_url,
    render,
)
from mpri_mcp.auth.admin import LoginRateLimiter, SetupTokenManager, admin_count
from mpri_mcp.config import Settings, get_settings
from mpri_mcp.container import Container
from mpri_mcp.db import Database
from mpri_mcp.log import configure_logging

log = logging.getLogger(__name__)

_HTML_PREFIXES = ("/admin", "/oauth")
_CSP = (
    "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; "
    "object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
)


@dataclass
class AppState:
    settings: Settings
    container: Container
    setup_tokens: SetupTokenManager
    login_limiter: LoginRateLimiter
    admin_exists: bool = False

    @property
    def db(self) -> Database:
        return self.container.db


class SecurityHeadersMiddleware:
    def __init__(self, app: ASGIApp, hsts: bool) -> None:
        self.app = app
        self.hsts = hsts

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        path: str = scope.get("path", "")

        async def send_with_headers(message: Message) -> None:
            if message["type"] == "http.response.start":
                headers = MutableHeaders(scope=message)
                headers.setdefault("X-Content-Type-Options", "nosniff")
                headers.setdefault("Referrer-Policy", "same-origin")
                if path.startswith(_HTML_PREFIXES) or path == "/":
                    headers.setdefault("Content-Security-Policy", _CSP)
                    headers.setdefault("X-Frame-Options", "DENY")
                if self.hsts:
                    headers.setdefault("Strict-Transport-Security", "max-age=31536000")
            await send(message)

        await self.app(scope, receive, send_with_headers)


def _session_secret(settings: Settings) -> str:
    if settings.session_secret:
        return settings.session_secret.get_secret_value()
    path: Path = settings.data_dir / "session_secret"
    if path.exists():
        return path.read_text(encoding="utf-8").strip()
    settings.data_dir.mkdir(parents=True, exist_ok=True)
    secret = secrets.token_urlsafe(48)
    path.write_text(secret, encoding="utf-8")
    try:
        path.chmod(0o600)
    except OSError:  # Windows / restricted filesystems
        pass
    return secret


def create_app(settings: Settings | None = None, container: Container | None = None) -> FastAPI:
    settings = settings or get_settings()
    configure_logging(settings.log_level, settings.log_json)
    configure_display(settings.scheduler_timezone)
    container = container or Container(settings)
    st = AppState(
        settings=settings,
        container=container,
        setup_tokens=SetupTokenManager(settings),
        login_limiter=LoginRateLimiter(),
    )

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        await container.start()
        st.admin_exists = await admin_count(container.db) > 0
        if not st.admin_exists:
            st.setup_tokens.issue()
        try:
            yield
        finally:
            await container.stop()

    app = FastAPI(
        title="mpri-mcp",
        version=__version__,
        lifespan=lifespan,
        docs_url=None,
        redoc_url=None,
        openapi_url=None,
    )
    app.state.mpri = st
    app.state.settings = settings

    app.add_middleware(
        SessionMiddleware,
        secret_key=_session_secret(settings),
        session_cookie="mpri_session",
        max_age=settings.session_idle_timeout_seconds,
        same_site="lax",
        https_only=settings.is_https,
    )
    app.add_middleware(SecurityHeadersMiddleware, hsts=settings.is_https)

    @app.exception_handler(SetupRequired)
    async def _setup_required(request: Request, exc: SetupRequired) -> Response:
        return RedirectResponse("/admin/setup", status_code=303)

    @app.exception_handler(LoginRequired)
    async def _login_required(request: Request, exc: LoginRequired) -> Response:
        return RedirectResponse(login_redirect_url(exc.next_url), status_code=303)

    @app.exception_handler(HTTPException)
    async def _http_error(request: Request, exc: HTTPException) -> Response:
        if request.url.path.startswith(_HTML_PREFIXES):
            titles = {403: "Not allowed", 404: "Not found"}
            return render(
                request,
                "error.html",
                status_code=exc.status_code,
                title=titles.get(exc.status_code, "Something went wrong"),
                message=exc.detail if exc.status_code != 404 else "This page does not exist.",
            )
        return JSONResponse({"detail": exc.detail}, status_code=exc.status_code)

    @app.get("/health", include_in_schema=False)
    async def health() -> JSONResponse:
        return JSONResponse({"status": "ok", "version": __version__})

    @app.get("/", include_in_schema=False)
    async def root() -> RedirectResponse:
        return RedirectResponse("/admin", status_code=303)

    app.mount("/static", StaticFiles(directory=STATIC_DIR), name="static")
    app.include_router(routes_auth.router)
    app.include_router(routes_pages.router)
    return app
