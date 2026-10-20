"""Anthropic Claude via the official SDK, using structured outputs (messages.parse)."""

from __future__ import annotations

import time
from typing import Any

import anthropic
from anthropic import AsyncAnthropic
from pydantic import BaseModel

from mpri_mcp.ai.base import LLMProvider, ProviderError, StructuredResult
from mpri_mcp.config import Settings

# Models that support server-side refusal fallback; Haiku has none (see Anthropic docs).
_SERVER_FALLBACK_MODELS = {
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
}
_FALLBACK_BETA = "server-side-fallback-2026-07-01"
# Small JSON answer, but thinking (always on for current models) counts toward max_tokens.
_MAX_TOKENS = 4096


class AnthropicProvider(LLMProvider):
    name = "anthropic"
    label = "Anthropic Claude"
    default_model = "claude-opus-5-5"
    suggested_models = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]

    def __init__(self, settings: Settings, client: Any | None = None) -> None:
        self._settings = settings
        self._client = client

    def is_configured(self) -> bool:
        return self._client is not None or self._settings.anthropic_api_key is not None

    def configuration_hint(self) -> str:
        return "Set ANTHROPIC_API_KEY."

    def _get_client(self) -> Any:
        if self._client is None:
            assert self._settings.anthropic_api_key is not None
            self._client = AsyncAnthropic(
                api_key=self._settings.anthropic_api_key.get_secret_value(),
                timeout=self._settings.ai_timeout_seconds,
                max_retries=0,  # the registry owns retries
            )
        return self._client

    async def generate_structured[T: BaseModel](
        self, *, system: str, prompt: str, schema: type[T], model: str
    ) -> StructuredResult[T]:
        params: dict[str, Any] = {
            "model": model,
            "max_tokens": _MAX_TOKENS,
            "system": system,
            "messages": [{"role": "user", "content": prompt}],
            "output_format": schema,
        }
        if self._settings.anthropic_effort:
            params["output_config"] = {"effort": self._settings.anthropic_effort}

        client = self._get_client()
        started = time.monotonic()
        try:
            if model in _SERVER_FALLBACK_MODELS:
                resp = await client.beta.messages.parse(
                    **params, betas=[_FALLBACK_BETA], fallbacks="default"
                )
            else:
                resp = await client.messages.parse(**params)
        except (
            anthropic.RateLimitError,
            anthropic.InternalServerError,
            anthropic.APITimeoutError,
            anthropic.APIConnectionError,
        ) as e:
            raise ProviderError(f"Anthropic API: {e}", retryable=True) from e
        except anthropic.APIStatusError as e:
            raise ProviderError(
                f"Anthropic API error {e.status_code}: {e.message}",
                retryable=e.status_code >= 500 or e.status_code == 408,
            ) from e

        if resp.stop_reason == "refusal":
            raise ProviderError("Claude declined to process this page (refusal).", retryable=False)
        if resp.stop_reason == "max_tokens":
            raise ProviderError("Claude hit max_tokens before finishing.", retryable=False)
        data = resp.parsed_output
        if data is None:
            raise ProviderError("Claude returned no structured output.", retryable=True)

        return StructuredResult(
            data=data,
            provider=self.name,
            model=resp.model or model,
            input_tokens=(resp.usage.input_tokens or 0)
            + (resp.usage.cache_read_input_tokens or 0)
            + (resp.usage.cache_creation_input_tokens or 0),
            output_tokens=resp.usage.output_tokens or 0,
            latency_ms=int((time.monotonic() - started) * 1000),
        )
