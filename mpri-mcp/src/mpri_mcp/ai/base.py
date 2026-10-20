"""Provider-neutral interface for structured (JSON-schema) generation."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, ClassVar

from pydantic import BaseModel


class ProviderError(Exception):
    """A provider call failed. `retryable` marks transient failures (429, 5xx, timeouts)."""

    def __init__(self, message: str, *, retryable: bool = False) -> None:
        super().__init__(message)
        self.retryable = retryable


class ProviderNotConfiguredError(ProviderError):
    pass


@dataclass
class StructuredResult[T: BaseModel]:
    data: T
    provider: str
    model: str
    input_tokens: int
    output_tokens: int
    latency_ms: int


class LLMProvider(ABC):
    name: ClassVar[str]
    label: ClassVar[str]
    default_model: ClassVar[str]
    suggested_models: ClassVar[list[str]] = []

    @abstractmethod
    def is_configured(self) -> bool: ...

    @abstractmethod
    def configuration_hint(self) -> str:
        """Which environment variables make this provider usable."""

    @abstractmethod
    async def generate_structured[T: BaseModel](
        self, *, system: str, prompt: str, schema: type[T], model: str
    ) -> StructuredResult[T]: ...


def strict_json_schema(model: type[BaseModel]) -> dict[str, Any]:
    """JSON schema in the strict subset OpenAI-style APIs require: every property required,
    no additional properties, no titles/defaults."""

    def fix(node: Any) -> Any:
        if isinstance(node, list):
            return [fix(v) for v in node]
        if not isinstance(node, dict):
            return node
        out: dict[str, Any] = {}
        for key, value in node.items():
            if key in ("title", "default"):
                continue
            if key in ("properties", "$defs"):
                out[key] = {name: fix(sub) for name, sub in value.items()}
            else:
                out[key] = fix(value)
        if out.get("type") == "object" and "properties" in out:
            out["required"] = list(out["properties"])
            out["additionalProperties"] = False
        return out

    return fix(model.model_json_schema())
