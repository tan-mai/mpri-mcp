"""Provider registry: picks provider+model, retries transient failures, falls back."""

from __future__ import annotations

import logging
from dataclasses import dataclass

from pydantic import BaseModel
from tenacity import (
    AsyncRetrying,
    retry_if_exception,
    stop_after_attempt,
    wait_exponential,
)
from tenacity.wait import wait_base

from mpri_mcp.ai.base import (
    LLMProvider,
    ProviderError,
    ProviderNotConfiguredError,
    StructuredResult,
)
from mpri_mcp.config import Settings

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class AIChoice:
    provider: str
    model: str

    def __str__(self) -> str:
        return f"{self.provider}/{self.model}"


@dataclass
class ProviderStatus:
    name: str
    label: str
    configured: bool
    default_model: str
    suggested_models: list[str]
    hint: str


def build_default_providers(settings: Settings) -> dict[str, LLMProvider]:
    from mpri_mcp.ai.anthropic import AnthropicProvider
    from mpri_mcp.ai.gemini import GeminiProvider
    from mpri_mcp.ai.openai_compat import OpenAICompatProvider

    providers: list[LLMProvider] = [
        GeminiProvider(settings),
        OpenAICompatProvider(settings),
        AnthropicProvider(settings),
    ]
    return {p.name: p for p in providers}


def _is_retryable(exc: BaseException) -> bool:
    return isinstance(exc, ProviderError) and exc.retryable


class ProviderRegistry:
    def __init__(
        self,
        settings: Settings,
        providers: dict[str, LLMProvider] | None = None,
        retry_wait: wait_base | None = None,
    ) -> None:
        self._settings = settings
        self._providers = providers if providers is not None else build_default_providers(settings)
        self._retry_wait = retry_wait or wait_exponential(multiplier=1, min=1, max=20)

    def names(self) -> list[str]:
        return list(self._providers)

    def get(self, name: str) -> LLMProvider:
        try:
            return self._providers[name]
        except KeyError:
            raise ProviderError(
                f"Unknown AI provider '{name}'. Available: {', '.join(self._providers)}"
            ) from None

    def statuses(self) -> list[ProviderStatus]:
        return [
            ProviderStatus(
                name=p.name,
                label=p.label,
                configured=p.is_configured(),
                default_model=p.default_model,
                suggested_models=list(p.suggested_models),
                hint=p.configuration_hint(),
            )
            for p in self._providers.values()
        ]

    # ------------------------------------------------------------------ choice resolution
    def resolve(self, levels: list[tuple[str | None, str | None]]) -> AIChoice:
        """Pick provider/model from (provider, model) levels ordered highest priority first,
        e.g. [supermarket override, dashboard setting, .env default].

        The provider is the first one set. A model applies if its level names no provider
        (it inherits) or names the chosen provider; otherwise the provider's default model."""
        provider = next((p for p, _ in levels if p), None) or self._settings.ai_provider
        for p, m in levels:
            if m and (p is None or p == provider):
                return AIChoice(provider, m)
        default = self._providers[provider].default_model if provider in self._providers else ""
        return AIChoice(provider, default)

    def resolve_fallback(self, levels: list[tuple[str | None, str | None]]) -> AIChoice | None:
        provider = next((p for p, _ in levels if p), None)
        if not provider:
            return None
        return self.resolve([(p, m) for p, m in levels if p in (None, provider)])

    # ------------------------------------------------------------------ calls
    async def _call[T: BaseModel](
        self, choice: AIChoice, *, system: str, prompt: str, schema: type[T]
    ) -> StructuredResult[T]:
        provider = self.get(choice.provider)
        if not provider.is_configured():
            hint = provider.configuration_hint()
            raise ProviderNotConfiguredError(
                f"AI provider '{choice.provider}' is not configured. {hint}"
            )
        retrying = AsyncRetrying(
            stop=stop_after_attempt(max(1, self._settings.ai_max_retries)),
            wait=self._retry_wait,
            retry=retry_if_exception(_is_retryable),
            reraise=True,
        )
        async for attempt in retrying:
            with attempt:
                try:
                    return await provider.generate_structured(
                        system=system, prompt=prompt, schema=schema, model=choice.model
                    )
                except TimeoutError as e:
                    raise ProviderError(f"{choice} timed out", retryable=True) from e
        raise AssertionError("unreachable")  # pragma: no cover

    async def generate_structured[T: BaseModel](
        self,
        *,
        system: str,
        prompt: str,
        schema: type[T],
        choice: AIChoice,
        fallback: AIChoice | None = None,
    ) -> StructuredResult[T]:
        try:
            return await self._call(choice, system=system, prompt=prompt, schema=schema)
        except ProviderError as primary:
            if fallback is None or fallback == choice:
                raise
            log.warning("AI %s failed (%s); trying fallback %s", choice, primary, fallback)
            try:
                return await self._call(fallback, system=system, prompt=prompt, schema=schema)
            except ProviderError as secondary:
                raise ProviderError(
                    f"{choice} failed: {primary}; fallback {fallback} failed: {secondary}"
                ) from secondary
