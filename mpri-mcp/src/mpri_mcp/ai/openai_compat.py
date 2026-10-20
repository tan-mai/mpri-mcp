"""OpenAI and any OpenAI-compatible Chat Completions API (OpenRouter, DeepSeek, Groq, Ollama, …)."""

from __future__ import annotations

import json
import time
from typing import Any

import openai
from openai import AsyncOpenAI
from pydantic import BaseModel, ValidationError

from mpri_mcp.ai.base import LLMProvider, ProviderError, StructuredResult, strict_json_schema
from mpri_mcp.config import Settings


class OpenAICompatProvider(LLMProvider):
    name = "openai"
    label = "OpenAI / OpenAI-compatible"
    default_model = "gpt-5-mini"
    suggested_models = ["gpt-5-mini", "gpt-5-nano"]

    def __init__(self, settings: Settings, client: Any | None = None) -> None:
        self._settings = settings
        self._client = client
        # Models/endpoints that rejected json_schema output; we fall back to JSON mode for them.
        self._json_mode_only: set[str] = set()

    def is_configured(self) -> bool:
        return self._client is not None or bool(
            self._settings.openai_api_key or self._settings.openai_base_url
        )

    def configuration_hint(self) -> str:
        return "Set OPENAI_API_KEY, and OPENAI_BASE_URL for a non-OpenAI compatible endpoint."

    def _get_client(self) -> Any:
        if self._client is None:
            key = self._settings.openai_api_key
            kwargs: dict[str, Any] = {
                # Local servers (Ollama, vLLM) accept any key.
                "api_key": key.get_secret_value() if key else "not-needed",
                "timeout": self._settings.ai_timeout_seconds,
                "max_retries": 0,  # the registry owns retries
            }
            if self._settings.openai_base_url:
                kwargs["base_url"] = self._settings.openai_base_url
            self._client = AsyncOpenAI(**kwargs)
        return self._client

    async def _create(self, **kwargs: Any) -> Any:
        try:
            return await self._get_client().chat.completions.create(**kwargs)
        except (openai.RateLimitError, openai.APITimeoutError, openai.APIConnectionError) as e:
            raise ProviderError(f"OpenAI-compatible API: {e}", retryable=True) from e
        except openai.APIStatusError as e:
            raise ProviderError(
                f"OpenAI-compatible API error {e.status_code}: {e.message}",
                retryable=e.status_code >= 500 or e.status_code == 408,
            ) from e

    async def generate_structured[T: BaseModel](
        self, *, system: str, prompt: str, schema: type[T], model: str
    ) -> StructuredResult[T]:
        json_schema = strict_json_schema(schema)
        base: dict[str, Any] = {"model": model}
        if self._settings.openai_temperature is not None:
            base["temperature"] = self._settings.openai_temperature

        started = time.monotonic()
        resp = None
        if model not in self._json_mode_only:
            try:
                resp = await self._create(
                    **base,
                    messages=[
                        {"role": "system", "content": system},
                        {"role": "user", "content": prompt},
                    ],
                    response_format={
                        "type": "json_schema",
                        "json_schema": {
                            "name": schema.__name__,
                            "schema": json_schema,
                            "strict": True,
                        },
                    },
                )
            except ProviderError as e:
                cause = e.__cause__
                if not (
                    isinstance(cause, openai.BadRequestError)
                    and ("response_format" in str(cause) or "json_schema" in str(cause))
                ):
                    raise
                self._json_mode_only.add(model)
        if resp is None:
            schema_hint = json.dumps(json_schema, ensure_ascii=False)
            resp = await self._create(
                **base,
                messages=[
                    {
                        "role": "system",
                        "content": f"{system}\n\nRespond with a single JSON object that "
                        f"matches this JSON schema:\n{schema_hint}",
                    },
                    {"role": "user", "content": prompt},
                ],
                response_format={"type": "json_object"},
            )

        if not resp.choices:
            raise ProviderError("OpenAI-compatible API returned no choices", retryable=True)
        message = resp.choices[0].message
        if getattr(message, "refusal", None):
            raise ProviderError(f"Model refused: {message.refusal}", retryable=False)
        try:
            data = schema.model_validate_json(message.content or "")
        except ValidationError as e:
            raise ProviderError(f"Model returned invalid JSON: {e}", retryable=True) from e

        usage = resp.usage
        return StructuredResult(
            data=data,
            provider=self.name,
            model=model,
            input_tokens=(usage.prompt_tokens or 0) if usage else 0,
            output_tokens=(usage.completion_tokens or 0) if usage else 0,
            latency_ms=int((time.monotonic() - started) * 1000),
        )
