"""Application settings, read from environment variables and an optional .env file."""

from __future__ import annotations

from functools import cached_property
from pathlib import Path
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

    # --- Scheduler ------------------------------------------------------------
    scheduler_timezone: str = "Asia/Ho_Chi_Minh"
    misfire_grace_seconds: int = 3600
    backup_cron: str = "30 4 * * *"
    backup_keep: int = 7

    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    # --- Agent crawl tasks (Claude reads the prices with the browser) ----------
    agent_task_lease_minutes: float = Field(
        30, gt=0, description="How long a claimed task stays reserved for one agent."
    )
    agent_task_expire_hours: float = Field(
        48, gt=0, description="A task still pending after this long is marked expired."
    )
    agent_task_max_attempts: int = Field(
        3, ge=1, description="Claims per task; an 'error' result re-queues it until this."
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

    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    @field_validator("database_url", mode="before")
    @classmethod
    def _empty_to_none(cls, v: object) -> object:
        return None if v == "" else v

    @field_validator("session_secret", "admin_setup_token", mode="before")
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
