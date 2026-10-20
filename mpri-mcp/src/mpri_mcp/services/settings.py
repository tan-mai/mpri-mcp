"""Runtime settings stored in the app_settings table (editable from the dashboard)."""

from __future__ import annotations

from dataclasses import asdict, dataclass
from typing import Any

from mpri_mcp.db import Database
from mpri_mcp.db.models import AppSetting

AI_KEY = "ai"
PRICES_KEY = "ai_prices"

# USD per 1M tokens, used only for the dashboard's cost estimate. Editable in AI settings.
DEFAULT_PRICES: dict[str, dict[str, float]] = {
    "gemini-2.5-flash-lite": {"input": 0.10, "output": 0.40},
    "gemini-3.1-flash-lite": {"input": 0.25, "output": 1.50},
    "gemini-3.5-flash-lite": {"input": 0.30, "output": 2.50},
    "gemini-2.5-flash": {"input": 0.30, "output": 2.50},
    "gemini-3.8-flash": {"input": 0.75, "output": 3.75},
}


@dataclass
class AISettings:
    """Dashboard overrides; None means "use the .env default"."""

    provider: str | None = None
    model: str | None = None
    fallback_provider: str | None = None
    fallback_model: str | None = None


async def get_value(db: Database, key: str, default: Any = None) -> Any:
    async with db.session() as s:
        row = await s.get(AppSetting, key)
    return default if row is None else row.value


async def set_value(db: Database, key: str, value: Any) -> None:
    async with db.session() as s:
        row = await s.get(AppSetting, key)
        if row is None:
            s.add(AppSetting(key=key, value=value))
        else:
            row.value = value


async def get_ai_settings(db: Database) -> AISettings:
    raw = await get_value(db, AI_KEY, {}) or {}
    return AISettings(**{k: raw.get(k) or None for k in AISettings.__dataclass_fields__})


async def set_ai_settings(db: Database, ai: AISettings) -> None:
    await set_value(db, AI_KEY, {k: (v or None) for k, v in asdict(ai).items()})


async def get_prices(db: Database) -> dict[str, dict[str, float]]:
    return await get_value(db, PRICES_KEY, None) or dict(DEFAULT_PRICES)


async def set_prices(db: Database, prices: dict[str, dict[str, float]]) -> None:
    await set_value(db, PRICES_KEY, prices)


def estimate_cost(
    prices: dict[str, dict[str, float]], model: str, input_tokens: int, output_tokens: int
) -> float | None:
    p = prices.get(model)
    if not p:
        return None
    return (input_tokens * p.get("input", 0) + output_tokens * p.get("output", 0)) / 1_000_000
