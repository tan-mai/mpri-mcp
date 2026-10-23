"""Helpers shared by MCP tool modules."""

from __future__ import annotations

import functools
from collections.abc import Awaitable, Callable
from datetime import datetime
from typing import TYPE_CHECKING, Any
from zoneinfo import ZoneInfo

from mcp.server.auth.middleware.auth_context import get_access_token
from mcp.server.mcpserver.exceptions import ToolError

from mpri_mcp.i18n import format_weekday_datetime, use_language
from mpri_mcp.services.errors import ServiceError

if TYPE_CHECKING:
    from mpri_mcp.container import Container


class Clock:
    def __init__(self, timezone: str) -> None:
        self.tz = ZoneInfo(timezone)
        self.name = timezone

    def iso(self, value: datetime | None) -> str | None:
        return value.astimezone(self.tz).isoformat(timespec="minutes") if value else None

    def human(self, value: datetime | None) -> str | None:
        """In the current language: 'Sat 24/10/2026 03:00' / 'Thứ Bảy, 24/10/2026 03:00'."""
        return format_weekday_datetime(value, self.name)


class ToolContext:
    """Wraps tool functions so each call runs in its connection's language."""

    def __init__(self, container: Container) -> None:
        self.c = container
        self.clock = Clock(container.settings.scheduler_timezone)

    async def current_language(self) -> str:
        token = get_access_token()  # None for unauthenticated/in-process calls
        if token is not None:
            language = await self.c.oauth.language_for_token(token.token)
            if language:
                return language
        return self.c.settings.default_language

    def tool[**P, R](self, fn: Callable[P, Awaitable[R]]) -> Callable[P, Awaitable[R]]:
        """Run in the connection's language; expected failures become readable ToolErrors."""

        @functools.wraps(fn)
        async def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
            with use_language(await self.current_language()):
                try:
                    return await fn(*args, **kwargs)
                except ServiceError as e:
                    raise ToolError(str(e)) from e

        return wrapper


def drop_none(d: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in d.items() if v is not None}
