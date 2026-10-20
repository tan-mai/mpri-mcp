"""Test doubles shared across test modules."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from pydantic import BaseModel

from mpri_mcp.ai.base import LLMProvider, ProviderError, StructuredResult


class FakeProvider(LLMProvider):
    """Returns canned data, or raises queued errors first."""

    name = "fake"
    label = "Fake"
    default_model = "fake-model"

    def __init__(
        self,
        responder: Callable[[str, str, type[BaseModel]], BaseModel | dict[str, Any]] | None = None,
        errors: list[ProviderError] | None = None,
        configured: bool = True,
        name: str | None = None,
    ) -> None:
        self.responder = responder
        self.errors = list(errors or [])
        self.configured = configured
        self.calls: list[dict[str, Any]] = []
        if name:
            self.name = name  # type: ignore[misc]

    def is_configured(self) -> bool:
        return self.configured

    def configuration_hint(self) -> str:
        return "Set FAKE_KEY."

    async def generate_structured[T: BaseModel](
        self, *, system: str, prompt: str, schema: type[T], model: str
    ) -> StructuredResult[T]:
        self.calls.append({"system": system, "prompt": prompt, "model": model})
        if self.errors:
            raise self.errors.pop(0)
        raw = self.responder(system, prompt, schema) if self.responder else {}
        data = raw if isinstance(raw, schema) else schema.model_validate(raw)
        return StructuredResult(
            data=data,
            provider=self.name,
            model=model,
            input_tokens=100,
            output_tokens=20,
            latency_ms=1,
        )
