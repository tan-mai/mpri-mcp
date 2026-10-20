"""Per-supermarket crawl configuration (stored as JSON in supermarkets.crawl_config)."""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field


class Cookie(BaseModel):
    model_config = ConfigDict(extra="forbid")

    name: str
    value: str
    domain: str | None = Field(None, description="Defaults to the supermarket's host.")
    path: str = "/"


class ProxyConfig(BaseModel):
    model_config = ConfigDict(extra="forbid")

    server: str = Field(..., description="e.g. http://vn-proxy.example.com:3128")
    username: str | None = None
    password: str | None = None


class SetupAction(BaseModel):
    """One browser step run once per crawl before visiting products, e.g. choosing a store."""

    model_config = ConfigDict(extra="forbid")

    action: Literal["goto", "click", "fill", "press", "wait", "wait_for"]
    selector: str | None = None
    value: str | None = None
    url: str | None = None
    ms: int | None = Field(None, ge=0, le=30000)


class CrawlConfig(BaseModel):
    model_config = ConfigDict(extra="forbid")

    wait_for_selector: str | None = Field(
        None, description="CSS selector that must appear before the page is read (e.g. the price)."
    )
    wait_ms: int = Field(0, ge=0, le=30000, description="Extra fixed wait after load.")
    text_selector: str | None = Field(
        None, description="CSS selector of the main product area; limits the text sent to the AI."
    )
    max_chars: int | None = Field(None, ge=1000, le=50000)
    cookies: list[Cookie] = Field(default_factory=list)
    extra_headers: dict[str, str] = Field(default_factory=dict)
    proxy: ProxyConfig | None = None
    block_resources: bool = Field(True, description="Skip images, fonts and media.")
    setup_url: str | None = Field(None, description="Page for setup_actions (default: base URL).")
    setup_actions: list[SetupAction] = Field(default_factory=list)
