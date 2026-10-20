"""Google Gemini via the google-genai SDK — Vertex AI or AI Studio backend."""

from __future__ import annotations

import time
from typing import Any

import httpx
import httpx2
from google import genai
from google.genai import errors as genai_errors
from google.genai import types
from pydantic import BaseModel, ValidationError

from mpri_mcp.ai.base import LLMProvider, ProviderError, StructuredResult
from mpri_mcp.config import Settings

_RETRYABLE_STATUS = {408, 429, 500, 502, 503, 504}
_NETWORK_ERRORS = (httpx.TransportError, httpx2.TransportError, TimeoutError, ConnectionError)


class GeminiProvider(LLMProvider):
    name = "gemini"
    label = "Google Gemini"
    default_model = "gemini-2.5-flash-lite"
    suggested_models = [
        "gemini-2.5-flash-lite",
        "gemini-3.1-flash-lite",
        "gemini-3.5-flash-lite",
        "gemini-3.8-flash",
    ]

    def __init__(self, settings: Settings, client: Any | None = None) -> None:
        self._settings = settings
        self._client = client

    @property
    def backend(self) -> str:
        return self._settings.gemini_backend

    def is_configured(self) -> bool:
        if self._client is not None:
            return True
        if self.backend == "vertex":
            return bool(self._settings.google_cloud_project)
        return self._settings.gemini_api_key is not None

    def configuration_hint(self) -> str:
        if self.backend == "vertex":
            return (
                "Set GOOGLE_CLOUD_PROJECT (and GOOGLE_CLOUD_LOCATION) and provide a "
                "service-account key via GOOGLE_APPLICATION_CREDENTIALS "
                "(./secrets/gcp-sa.json in Docker)."
            )
        return "Set GEMINI_API_KEY (AI Studio)."

    def _get_client(self) -> Any:
        if self._client is None:
            http = types.HttpOptions(timeout=int(self._settings.ai_timeout_seconds * 1000))
            if self.backend == "vertex":
                self._client = genai.Client(
                    vertexai=True,
                    project=self._settings.google_cloud_project,
                    location=self._settings.google_cloud_location,
                    http_options=http,
                )
            else:
                assert self._settings.gemini_api_key is not None
                self._client = genai.Client(
                    api_key=self._settings.gemini_api_key.get_secret_value(), http_options=http
                )
        return self._client

    async def generate_structured[T: BaseModel](
        self, *, system: str, prompt: str, schema: type[T], model: str
    ) -> StructuredResult[T]:
        config = types.GenerateContentConfig(
            system_instruction=system,
            response_mime_type="application/json",
            response_schema=schema,
            temperature=0,
        )
        if self._settings.gemini_thinking_budget is not None:
            config.thinking_config = types.ThinkingConfig(
                thinking_budget=self._settings.gemini_thinking_budget
            )
        started = time.monotonic()
        try:
            resp = await self._get_client().aio.models.generate_content(
                model=model, contents=prompt, config=config
            )
        except genai_errors.APIError as e:
            raise ProviderError(
                f"Gemini API error {e.code}: {e.message or e.status}",
                retryable=e.code in _RETRYABLE_STATUS,
            ) from e
        except _NETWORK_ERRORS as e:
            raise ProviderError(f"Gemini request failed: {e!r}", retryable=True) from e

        data = resp.parsed if isinstance(resp.parsed, schema) else None
        if data is None:
            text = resp.text
            if not text:
                reason = resp.candidates[0].finish_reason if resp.candidates else "no candidates"
                raise ProviderError(f"Gemini returned no content ({reason})", retryable=False)
            try:
                data = schema.model_validate_json(text)
            except ValidationError as e:
                raise ProviderError(f"Gemini returned invalid JSON: {e}", retryable=True) from e

        usage = resp.usage_metadata
        return StructuredResult(
            data=data,
            provider=self.name,
            model=model,
            input_tokens=(usage.prompt_token_count or 0) if usage else 0,
            output_tokens=(
                (usage.candidates_token_count or 0) + (usage.thoughts_token_count or 0)
                if usage
                else 0
            ),
            latency_ms=int((time.monotonic() - started) * 1000),
        )
