"""SQLAlchemy ORM models."""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Any

from sqlalchemy import (
    JSON,
    Boolean,
    DateTime,
    ForeignKey,
    Index,
    Integer,
    String,
    Text,
    TypeDecorator,
    UniqueConstraint,
)
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column, relationship

from mpri_mcp.utils.time import utcnow


class UTCDateTime(TypeDecorator[datetime]):
    """Stores naive UTC (SQLite has no timezone support), always returns aware UTC."""

    impl = DateTime
    cache_ok = True

    def process_bind_param(self, value: datetime | None, dialect: Any) -> datetime | None:
        if value is None:
            return None
        if value.tzinfo is None:
            raise ValueError("naive datetime given to UTCDateTime column")
        return value.astimezone(UTC).replace(tzinfo=None)

    def process_result_value(self, value: datetime | None, dialect: Any) -> datetime | None:
        return None if value is None else value.replace(tzinfo=UTC)


class Base(DeclarativeBase):
    type_annotation_map = {datetime: UTCDateTime, dict[str, Any]: JSON, list[str]: JSON}


# --------------------------------------------------------------------------- Domain


class Supermarket(Base):
    __tablename__ = "supermarkets"

    id: Mapped[str] = mapped_column(String(64), primary_key=True)
    name: Mapped[str] = mapped_column(String(200), unique=True)
    base_url: Mapped[str] = mapped_column(String(500))
    cron_expression: Mapped[str | None] = mapped_column(String(100))
    cron_enabled: Mapped[bool] = mapped_column(Boolean, default=False)
    timezone: Mapped[str] = mapped_column(String(64), default="Asia/Ho_Chi_Minh")
    crawl_config: Mapped[dict[str, Any]] = mapped_column(default=dict)
    ai_provider: Mapped[str | None] = mapped_column(String(50))
    ai_model: Mapped[str | None] = mapped_column(String(100))
    created_at: Mapped[datetime] = mapped_column(default=utcnow)
    updated_at: Mapped[datetime] = mapped_column(default=utcnow, onupdate=utcnow)

    urls: Mapped[list[ProductUrl]] = relationship(
        back_populates="supermarket", cascade="all, delete-orphan", passive_deletes=True
    )


class Product(Base):
    __tablename__ = "products"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    name: Mapped[str] = mapped_column(String(300))
    normalized_name: Mapped[str] = mapped_column(String(300), unique=True, index=True)
    category: Mapped[str | None] = mapped_column(String(100))
    created_at: Mapped[datetime] = mapped_column(default=utcnow)

    urls: Mapped[list[ProductUrl]] = relationship(
        back_populates="product", cascade="all, delete-orphan", passive_deletes=True
    )


class ProductUrl(Base):
    __tablename__ = "product_urls"
    __table_args__ = (UniqueConstraint("product_id", "supermarket_id"),)

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    product_id: Mapped[int] = mapped_column(ForeignKey("products.id", ondelete="CASCADE"))
    supermarket_id: Mapped[str] = mapped_column(
        ForeignKey("supermarkets.id", ondelete="CASCADE"), index=True
    )
    url: Mapped[str] = mapped_column(String(1000))
    is_active: Mapped[bool] = mapped_column(Boolean, default=True)
    last_crawled_at: Mapped[datetime | None]
    last_success_at: Mapped[datetime | None]
    last_error: Mapped[str | None] = mapped_column(Text)
    consecutive_failures: Mapped[int] = mapped_column(Integer, default=0)
    created_at: Mapped[datetime] = mapped_column(default=utcnow)

    product: Mapped[Product] = relationship(back_populates="urls")
    supermarket: Mapped[Supermarket] = relationship(back_populates="urls")
    prices: Mapped[list[PriceRecord]] = relationship(
        back_populates="product_url", cascade="all, delete-orphan", passive_deletes=True
    )


class PriceRecord(Base):
    __tablename__ = "price_records"
    __table_args__ = (Index("ix_price_records_url_time", "product_url_id", "crawled_at"),)

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    product_url_id: Mapped[int] = mapped_column(ForeignKey("product_urls.id", ondelete="CASCADE"))
    current_price: Mapped[int] = mapped_column(Integer)
    original_price: Mapped[int | None] = mapped_column(Integer)
    member_price: Mapped[int | None] = mapped_column(Integer)
    in_stock: Mapped[bool | None] = mapped_column(Boolean)
    crawled_at: Mapped[datetime] = mapped_column(default=utcnow, index=True)
    provider: Mapped[str | None] = mapped_column(String(50))
    model: Mapped[str | None] = mapped_column(String(100))
    raw_extraction: Mapped[dict[str, Any] | None] = mapped_column(JSON)

    product_url: Mapped[ProductUrl] = relationship(back_populates="prices")


# --------------------------------------------------------------------------- Crawl runs


class CrawlRun(Base):
    __tablename__ = "crawl_runs"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    supermarket_id: Mapped[str] = mapped_column(
        ForeignKey("supermarkets.id", ondelete="CASCADE"), index=True
    )
    trigger: Mapped[str] = mapped_column(String(20))  # cron | manual
    status: Mapped[str] = mapped_column(String(20), default="running")
    # running | success | partial | failed | timeout
    started_at: Mapped[datetime] = mapped_column(default=utcnow, index=True)
    finished_at: Mapped[datetime | None]
    urls_total: Mapped[int] = mapped_column(Integer, default=0)
    urls_ok: Mapped[int] = mapped_column(Integer, default=0)
    urls_failed: Mapped[int] = mapped_column(Integer, default=0)
    error: Mapped[str | None] = mapped_column(Text)

    items: Mapped[list[CrawlRunItem]] = relationship(
        back_populates="run", cascade="all, delete-orphan", passive_deletes=True
    )


class CrawlRunItem(Base):
    __tablename__ = "crawl_run_items"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    run_id: Mapped[int] = mapped_column(ForeignKey("crawl_runs.id", ondelete="CASCADE"), index=True)
    product_url_id: Mapped[int | None] = mapped_column(
        ForeignKey("product_urls.id", ondelete="SET NULL")
    )
    url: Mapped[str] = mapped_column(String(1000))
    status: Mapped[str] = mapped_column(String(20))  # ok | failed
    error: Mapped[str | None] = mapped_column(Text)
    duration_ms: Mapped[int | None] = mapped_column(Integer)
    provider: Mapped[str | None] = mapped_column(String(50))
    model: Mapped[str | None] = mapped_column(String(100))
    input_tokens: Mapped[int | None] = mapped_column(Integer)
    output_tokens: Mapped[int | None] = mapped_column(Integer)
    created_at: Mapped[datetime] = mapped_column(default=utcnow, index=True)

    run: Mapped[CrawlRun] = relationship(back_populates="items")


# --------------------------------------------------------------------------- Auth


class AdminUser(Base):
    __tablename__ = "admin_users"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    username: Mapped[str] = mapped_column(String(100), unique=True)
    password_hash: Mapped[str] = mapped_column(String(300))
    # Bumped on password change / "log out everywhere"; sessions carry the value they saw.
    session_version: Mapped[int] = mapped_column(Integer, default=1)
    created_at: Mapped[datetime] = mapped_column(default=utcnow)
    last_login_at: Mapped[datetime | None]
    password_changed_at: Mapped[datetime] = mapped_column(default=utcnow)


class OAuthClient(Base):
    __tablename__ = "oauth_clients"

    client_id: Mapped[str] = mapped_column(String(100), primary_key=True)
    # Stored as issued: the SDK's client authenticator compares it directly. A leaked secret
    # alone is useless — every grant also needs a PKCE-bound code or a (hashed) refresh token.
    client_secret: Mapped[str | None] = mapped_column(String(200))
    client_name: Mapped[str | None] = mapped_column(String(200))
    info: Mapped[dict[str, Any]] = mapped_column(JSON)  # full RFC 7591 registration record
    created_at: Mapped[datetime] = mapped_column(default=utcnow)
    last_used_at: Mapped[datetime | None]
    revoked_at: Mapped[datetime | None]


class OAuthPendingAuthorization(Base):
    """An /authorize request waiting for the admin to log in and approve it."""

    __tablename__ = "oauth_pending_authorizations"

    id: Mapped[str] = mapped_column(String(64), primary_key=True)
    client_id: Mapped[str] = mapped_column(
        ForeignKey("oauth_clients.client_id", ondelete="CASCADE")
    )
    redirect_uri: Mapped[str] = mapped_column(String(1000))
    redirect_uri_provided_explicitly: Mapped[bool] = mapped_column(Boolean)
    state: Mapped[str | None] = mapped_column(String(500))
    code_challenge: Mapped[str] = mapped_column(String(200))
    scopes: Mapped[list[str]] = mapped_column(JSON)
    resource: Mapped[str | None] = mapped_column(String(500))
    created_at: Mapped[datetime] = mapped_column(default=utcnow)
    expires_at: Mapped[datetime]


class OAuthAuthorizationCode(Base):
    __tablename__ = "oauth_authorization_codes"

    code_hash: Mapped[str] = mapped_column(String(64), primary_key=True)
    client_id: Mapped[str] = mapped_column(
        ForeignKey("oauth_clients.client_id", ondelete="CASCADE")
    )
    redirect_uri: Mapped[str] = mapped_column(String(1000))
    redirect_uri_provided_explicitly: Mapped[bool] = mapped_column(Boolean)
    code_challenge: Mapped[str] = mapped_column(String(200))
    scopes: Mapped[list[str]] = mapped_column(JSON)
    resource: Mapped[str | None] = mapped_column(String(500))
    subject: Mapped[str | None] = mapped_column(String(100))
    family_id: Mapped[str] = mapped_column(String(64))
    created_at: Mapped[datetime] = mapped_column(default=utcnow)
    expires_at: Mapped[datetime]
    used_at: Mapped[datetime | None]


class OAuthToken(Base):
    __tablename__ = "oauth_tokens"

    token_hash: Mapped[str] = mapped_column(String(64), primary_key=True)
    kind: Mapped[str] = mapped_column(String(10))  # access | refresh
    client_id: Mapped[str] = mapped_column(
        ForeignKey("oauth_clients.client_id", ondelete="CASCADE"), index=True
    )
    scopes: Mapped[list[str]] = mapped_column(JSON)
    resource: Mapped[str | None] = mapped_column(String(500))
    subject: Mapped[str | None] = mapped_column(String(100))
    # All tokens descending from one authorization code share a family; reuse of a rotated
    # refresh token (or of the code) revokes the whole family.
    family_id: Mapped[str] = mapped_column(String(64), index=True)
    created_at: Mapped[datetime] = mapped_column(default=utcnow)
    expires_at: Mapped[datetime]
    used_at: Mapped[datetime | None]  # refresh tokens: set when rotated
    revoked_at: Mapped[datetime | None]
    last_used_at: Mapped[datetime | None]


class OAuthGrant(Base):
    """One connection (one approved authorization, i.e. one token family) and its preferences.

    A new table rather than columns on existing ones, so older databases need no migration."""

    __tablename__ = "oauth_grants"

    family_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    client_id: Mapped[str] = mapped_column(
        ForeignKey("oauth_clients.client_id", ondelete="CASCADE"), index=True
    )
    subject: Mapped[str | None] = mapped_column(String(100))
    language: Mapped[str] = mapped_column(String(8), default="en")
    created_at: Mapped[datetime] = mapped_column(default=utcnow)
    updated_at: Mapped[datetime] = mapped_column(default=utcnow, onupdate=utcnow)


# --------------------------------------------------------------------------- Settings


class AppSetting(Base):
    __tablename__ = "app_settings"

    key: Mapped[str] = mapped_column(String(100), primary_key=True)
    value: Mapped[Any] = mapped_column(JSON)
    updated_at: Mapped[datetime] = mapped_column(default=utcnow, onupdate=utcnow)
