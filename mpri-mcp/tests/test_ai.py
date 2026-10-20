import json
from types import SimpleNamespace

import anthropic
import httpx2
import openai
import pytest
from google.genai import errors as genai_errors
from pydantic import BaseModel
from tenacity import wait_none

from mpri_mcp.ai.anthropic import AnthropicProvider
from mpri_mcp.ai.base import ProviderError, ProviderNotConfiguredError, strict_json_schema
from mpri_mcp.ai.gemini import GeminiProvider
from mpri_mcp.ai.openai_compat import OpenAICompatProvider
from mpri_mcp.ai.registry import AIChoice, ProviderRegistry, build_default_providers
from tests.fakes import FakeProvider


class Price(BaseModel):
    title: str | None = None  # a property literally named "title"
    current_price: int | None
    in_stock: bool | None = None


# --------------------------------------------------------------------------- schema


def test_strict_json_schema_keeps_property_named_title():
    schema = strict_json_schema(Price)
    assert set(schema["properties"]) == {"title", "current_price", "in_stock"}
    assert schema["required"] == ["title", "current_price", "in_stock"]
    assert schema["additionalProperties"] is False
    assert "title" not in schema["properties"]["current_price"]
    assert "default" not in schema["properties"]["title"]


# --------------------------------------------------------------------------- gemini


class _GeminiModels:
    def __init__(self, result=None, error=None):
        self.result, self.error, self.kwargs = result, error, None

    async def generate_content(self, **kwargs):
        self.kwargs = kwargs
        if self.error:
            raise self.error
        return self.result


def _gemini(settings, models):
    client = SimpleNamespace(aio=SimpleNamespace(models=models))
    return GeminiProvider(settings, client=client)


def _gemini_response(parsed=None, text=None):
    return SimpleNamespace(
        parsed=parsed,
        text=text,
        candidates=[],
        usage_metadata=SimpleNamespace(
            prompt_token_count=120, candidates_token_count=15, thoughts_token_count=5
        ),
    )


async def test_gemini_parsed_result(settings):
    models = _GeminiModels(_gemini_response(parsed=Price(current_price=95000)))
    result = await _gemini(settings, models).generate_structured(
        system="s", prompt="p", schema=Price, model="gemini-2.5-flash-lite"
    )
    assert result.data.current_price == 95000
    assert (result.input_tokens, result.output_tokens) == (120, 20)
    config = models.kwargs["config"]
    assert config.response_schema is Price and config.temperature == 0
    assert config.thinking_config is None


async def test_gemini_text_fallback_and_thinking_budget(settings):
    settings.gemini_thinking_budget = 0
    models = _GeminiModels(_gemini_response(text='{"current_price": 1000}'))
    result = await _gemini(settings, models).generate_structured(
        system="s", prompt="p", schema=Price, model="m"
    )
    assert result.data.current_price == 1000
    assert models.kwargs["config"].thinking_config.thinking_budget == 0


@pytest.mark.parametrize(("code", "retryable"), [(429, True), (503, True), (400, False)])
async def test_gemini_errors(settings, code, retryable):
    err = genai_errors.APIError(code, {"error": {"message": "boom", "status": "X"}})
    with pytest.raises(ProviderError) as exc:
        await _gemini(settings, _GeminiModels(error=err)).generate_structured(
            system="s", prompt="p", schema=Price, model="m"
        )
    assert exc.value.retryable is retryable


def test_gemini_configuration(settings):
    settings.gemini_backend = "vertex"
    settings.google_cloud_project = None
    assert not GeminiProvider(settings).is_configured()
    settings.google_cloud_project = "my-project"
    assert GeminiProvider(settings).is_configured()


# --------------------------------------------------------------------------- openai


class _Completions:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    async def create(self, **kwargs):
        self.calls.append(kwargs)
        item = self.responses.pop(0)
        if isinstance(item, Exception):
            raise item
        return item


def _openai_response(content, refusal=None):
    return SimpleNamespace(
        choices=[SimpleNamespace(message=SimpleNamespace(content=content, refusal=refusal))],
        usage=SimpleNamespace(prompt_tokens=50, completion_tokens=7),
    )


def _openai(settings, completions):
    client = SimpleNamespace(chat=SimpleNamespace(completions=completions))
    return OpenAICompatProvider(settings, client=client)


async def test_openai_json_schema(settings):
    completions = _Completions([_openai_response(json.dumps({"current_price": 5}))])
    result = await _openai(settings, completions).generate_structured(
        system="s", prompt="p", schema=Price, model="gpt-x"
    )
    assert result.data.current_price == 5 and result.output_tokens == 7
    fmt = completions.calls[0]["response_format"]
    assert fmt["type"] == "json_schema" and fmt["json_schema"]["strict"] is True
    assert "temperature" not in completions.calls[0]


async def test_openai_falls_back_to_json_mode(settings):
    bad = openai.BadRequestError(
        "response_format json_schema is not supported",
        response=httpx2.Response(400, request=httpx2.Request("POST", "http://x")),
        body=None,
    )
    completions = _Completions(
        [bad, _openai_response('{"current_price": 7}'), _openai_response('{"current_price": 8}')]
    )
    provider = _openai(settings, completions)
    first = await provider.generate_structured(system="s", prompt="p", schema=Price, model="llama")
    assert first.data.current_price == 7
    assert completions.calls[1]["response_format"] == {"type": "json_object"}
    assert "JSON schema" in completions.calls[1]["messages"][0]["content"]
    # Remembered: next call goes straight to JSON mode.
    await provider.generate_structured(system="s", prompt="p", schema=Price, model="llama")
    assert completions.calls[2]["response_format"] == {"type": "json_object"}


async def test_openai_refusal_and_rate_limit(settings):
    refusal = _Completions([_openai_response(None, refusal="no")])
    with pytest.raises(ProviderError) as exc:
        await _openai(settings, refusal).generate_structured(
            system="s", prompt="p", schema=Price, model="m"
        )
    assert not exc.value.retryable

    limited = openai.RateLimitError(
        "slow down",
        response=httpx2.Response(429, request=httpx2.Request("POST", "http://x")),
        body=None,
    )
    with pytest.raises(ProviderError) as exc:
        await _openai(settings, _Completions([limited])).generate_structured(
            system="s", prompt="p", schema=Price, model="m"
        )
    assert exc.value.retryable


# --------------------------------------------------------------------------- anthropic


class _Parser:
    def __init__(self, result=None, error=None):
        self.result, self.error, self.calls = result, error, []

    async def parse(self, **kwargs):
        self.calls.append(kwargs)
        if self.error:
            raise self.error
        return self.result


def _claude_response(parsed, stop_reason="end_turn"):
    return SimpleNamespace(
        stop_reason=stop_reason,
        parsed_output=parsed,
        model="claude-x",
        usage=SimpleNamespace(
            input_tokens=300,
            output_tokens=40,
            cache_read_input_tokens=0,
            cache_creation_input_tokens=0,
        ),
    )


def _anthropic(settings, messages, beta_messages):
    client = SimpleNamespace(messages=messages, beta=SimpleNamespace(messages=beta_messages))
    return AnthropicProvider(settings, client=client)


async def test_anthropic_opus_uses_server_fallback(settings):
    plain, beta = _Parser(), _Parser(_claude_response(Price(current_price=9)))
    result = await _anthropic(settings, plain, beta).generate_structured(
        system="s", prompt="p", schema=Price, model="claude-opus-5-5"
    )
    assert result.data.current_price == 9 and result.input_tokens == 300
    call = beta.calls[0]
    assert call["fallbacks"] == "default"
    assert call["betas"] == ["server-side-fallback-2026-07-01"]
    assert call["output_format"] is Price
    assert call["output_config"] == {"effort": "low"}
    assert plain.calls == []


async def test_anthropic_haiku_uses_plain_parse(settings):
    settings.anthropic_effort = None
    plain, beta = _Parser(_claude_response(Price(current_price=1))), _Parser()
    await _anthropic(settings, plain, beta).generate_structured(
        system="s", prompt="p", schema=Price, model="claude-haiku-5-5"
    )
    assert beta.calls == [] and "fallbacks" not in plain.calls[0]
    assert "output_config" not in plain.calls[0]


async def test_anthropic_refusal_is_not_retryable(settings):
    plain = _Parser(_claude_response(None, stop_reason="refusal"))
    with pytest.raises(ProviderError) as exc:
        await _anthropic(settings, plain, _Parser()).generate_structured(
            system="s", prompt="p", schema=Price, model="claude-haiku-5-5"
        )
    assert not exc.value.retryable


async def test_anthropic_overloaded_is_retryable(settings):
    err = anthropic.InternalServerError(
        "overloaded",
        response=httpx2.Response(529, request=httpx2.Request("POST", "http://x")),
        body=None,
    )
    with pytest.raises(ProviderError) as exc:
        await _anthropic(settings, _Parser(error=err), _Parser()).generate_structured(
            system="s", prompt="p", schema=Price, model="claude-haiku-5-5"
        )
    assert exc.value.retryable


# --------------------------------------------------------------------------- registry


def _registry(settings, **providers):
    return ProviderRegistry(settings, providers=providers, retry_wait=wait_none())


def test_default_providers_registered(settings):
    assert set(build_default_providers(settings)) == {"gemini", "openai", "anthropic"}


def test_resolve_priority(settings):
    reg = ProviderRegistry(settings)
    env = ("gemini", "gemini-2.5-flash-lite")
    assert reg.resolve([(None, None), (None, None), env]) == AIChoice(*env)
    # Dashboard switches provider without a model -> that provider's default model.
    assert reg.resolve([(None, None), ("openai", None), env]) == AIChoice("openai", "gpt-5-mini")
    # Supermarket sets only a model -> inherits the provider below it.
    assert reg.resolve([(None, "gemini-3.5-flash-lite"), (None, None), env]) == AIChoice(
        "gemini", "gemini-3.5-flash-lite"
    )
    assert reg.resolve([("anthropic", "claude-haiku-5-5"), ("openai", "gpt-x"), env]) == AIChoice(
        "anthropic", "claude-haiku-5-5"
    )
    assert reg.resolve_fallback([(None, None), (None, None)]) is None
    assert reg.resolve_fallback([(None, None), ("openai", None)]) == AIChoice(
        "openai", "gpt-5-mini"
    )


async def test_registry_retries_transient_errors(settings):
    fake = FakeProvider(
        responder=lambda *_: {"current_price": 1},
        errors=[ProviderError("429", retryable=True), ProviderError("503", retryable=True)],
    )
    reg = _registry(settings, fake=fake)
    result = await reg.generate_structured(
        system="s", prompt="p", schema=Price, choice=AIChoice("fake", "m")
    )
    assert result.data.current_price == 1 and len(fake.calls) == 3


async def test_registry_does_not_retry_permanent_errors(settings):
    fake = FakeProvider(errors=[ProviderError("bad request", retryable=False)])
    with pytest.raises(ProviderError):
        await _registry(settings, fake=fake).generate_structured(
            system="s", prompt="p", schema=Price, choice=AIChoice("fake", "m")
        )
    assert len(fake.calls) == 1


async def test_registry_uses_fallback(settings):
    primary = FakeProvider(errors=[ProviderError("quota", retryable=False)], name="primary")
    backup = FakeProvider(responder=lambda *_: {"current_price": 2}, name="backup")
    reg = _registry(settings, primary=primary, backup=backup)
    result = await reg.generate_structured(
        system="s",
        prompt="p",
        schema=Price,
        choice=AIChoice("primary", "m1"),
        fallback=AIChoice("backup", "m2"),
    )
    assert result.provider == "backup" and result.model == "m2"


async def test_registry_unconfigured_provider(settings):
    reg = _registry(settings, fake=FakeProvider(configured=False))
    with pytest.raises(ProviderNotConfiguredError, match="FAKE_KEY"):
        await reg.generate_structured(
            system="s", prompt="p", schema=Price, choice=AIChoice("fake", "m")
        )
    with pytest.raises(ProviderError, match="Unknown AI provider"):
        reg.get("nope")
