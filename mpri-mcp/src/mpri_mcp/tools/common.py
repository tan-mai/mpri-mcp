"""Helpers shared by MCP tool modules."""

from __future__ import annotations

import functools
from collections.abc import Awaitable, Callable
from datetime import datetime
from typing import Any
from zoneinfo import ZoneInfo

from mcp.server.mcpserver.exceptions import ToolError

from mpri_mcp.services.errors import ServiceError


def service_errors[**P, R](fn: Callable[P, Awaitable[R]]) -> Callable[P, Awaitable[R]]:
    """Turn expected service failures into ToolError so Claude sees the message."""

    @functools.wraps(fn)
    async def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
        try:
            return await fn(*args, **kwargs)
        except ServiceError as e:
            raise ToolError(str(e)) from e

    return wrapper


class Clock:
    def __init__(self, timezone: str) -> None:
        self.tz = ZoneInfo(timezone)
        self.name = timezone

    def iso(self, value: datetime | None) -> str | None:
        return value.astimezone(self.tz).isoformat(timespec="minutes") if value else None

    def human(self, value: datetime | None) -> str | None:
        return value.astimezone(self.tz).strftime("%a %d/%m/%Y %H:%M") if value else None


def drop_none(d: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in d.items() if v is not None}
