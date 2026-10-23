"""Application settings, read from environment variables and an optional .env file."""

from __future__ import annotations

from functools import cached_property
from pathlib import Path
from typing import Literal
from urllib.parse import urlparse

from pydantic import Field, SecretStr, field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_file_encoding="utf-8", extra="ignore")

    # --- Server -------------------------------------------------------------
    public_base_url: str = Field(
        "http://localhost:8000",
        description="Public origin users and Claude reach, e.g. https://mpri.example.com. "
        "Used as the OAuth issuer and to build the MCP resource URL.",
    )
    host: str = "0.0.0.0"
    port: int = 8000
    forwarded_allow_ips: str = Field(
        "127.0.0.1",
        description="Proxies whose X-Forwarded-* headers uvicorn trusts. '*' is fine when the app "
        "port is only reachable through the reverse proxy (as in compose.yaml).",
    )
    data_dir: Path = Path("data")
    database_url: str | None = None
    log_level: str = "INFO"
    log_json: bool = False
    default_language: Literal["en", "vi"] = Field(
        "en",
        description="Language for the dashboard and MCP text until a user picks another one.",
    )

    # --- Scheduler / crawler -----------------------------------------------
    scheduler_timezone: str = "Asia/Ho_Chi_Minh"
    max_concurrent_markets: int = 2
    max_pages_per_market: int = 3
    page_timeout_seconds: int = 45
    job_timeout_seconds: int = 1800
    misfire_grace_seconds: int = 3600
    backup_cron: str = "30 4 * * *"
    backup_keep: int = 7
    browser_headless: bool = True
    browser_user_agent: str = (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
        "(KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36"
    )
    page_text_max_chars: int = 8000

    # --- AI providers ---------------------------------------------------------
    ai_provider: str = "gemini"
    ai_model: str = "gemini-2.5-flash-lite"
    ai_fallback_provider: str | None = None
    ai_fallback_model: str | None = None
    ai_timeout_seconds: float = 60
    ai_max_retries: int = 3

    gemini_backend: Literal["vertex", "studio"] = "vertex"
    google_cloud_project: str | None = None
    google_cloud_location: str = "global"
    gemini_api_key: SecretStr | None = None
    gemini_thinking_budget: int | None = None

    openai_api_key: SecretStr | None = None
    openai_base_url: str | None = None
    openai_temperature: float | None = None

    anthropic_api_key: SecretStr | None = None
    anthropic_effort: str | None = Field(
        "low", description="output_config.effort for Claude models; empty to omit (older models)."
    )

    # --- Auth -----------------------------------------------------------------
    session_secret: SecretStr | None = Field(
        None, description="Signs admin session cookies. Generated into DATA_DIR if unset."
    )
    session_idle_timeout_seconds: int = 12 * 3600
    admin_setup_token: SecretStr | None = Field(
        None, description="Fixed first-run setup token. A random one is logged if unset."
    )
    oauth_scope: str = "mpri"
    oauth_access_token_ttl_seconds: int = 3600
    oauth_refresh_token_ttl_seconds: int = 30 * 86400
    oauth_code_ttl_seconds: int = 300
    oauth_allowed_redirect_uris: list[str] = [
        "https://claude.ai/api/mcp/auth_callback",
        "https://claude.com/api/mcp/auth_callback",
    ]
    oauth_allow_loopback_redirects: bool = True

    @field_validator("public_base_url")
    @classmethod
    def _strip_trailing_slash(cls, v: str) -> str:
        return v.rstrip("/")

    @field_validator(
        "ai_fallback_provider",
        "ai_fallback_model",
        "google_cloud_project",
        "openai_base_url",
        "database_url",
        "anthropic_effort",
        mode="before",
    )
    @classmethod
    def _empty_to_none(cls, v: object) -> object:
        return None if v == "" else v

    @field_validator(
        "gemini_api_key",
        "openai_api_key",
        "anthropic_api_key",
        "session_secret",
        "admin_setup_token",
        "gemini_thinking_budget",
        "openai_temperature",
        mode="before",
    )
    @classmethod
    def _empty_secret_to_none(cls, v: object) -> object:
        return None if v == "" else v

    # --- Derived values ---------------------------------------------------------
    @cached_property
    def resolved_database_url(self) -> str:
        if self.database_url:
            return self.database_url
        return f"sqlite+aiosqlite:///{(self.data_dir / 'mpri.db').as_posix()}"

    @cached_property
    def issuer_url(self) -> str:
        return self.public_base_url

    @cached_property
    def mcp_resource_url(self) -> str:
        return f"{self.public_base_url}/mcp"

    @cached_property
    def public_host(self) -> str:
        return urlparse(self.public_base_url).netloc

    @cached_property
    def is_https(self) -> bool:
        return self.public_base_url.startswith("https://")


def get_settings() -> Settings:
    return Settings()
