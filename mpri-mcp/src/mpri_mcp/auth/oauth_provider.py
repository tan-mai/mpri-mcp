"""OAuth 2.1 authorization server state in SQLite, plugged into the MCP SDK's AS handlers.

The SDK supplies the protocol endpoints (/authorize, /token with PKCE, /register, /revoke);
this provider supplies storage, policy (redirect allow-list, single-use codes, refresh token
rotation with reuse detection) and the hand-off to our own login + consent page.
"""

from __future__ import annotations

import hashlib
import logging
import secrets
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Any
from urllib.parse import urlparse

from mcp.server.auth.provider import (
    AccessToken,
    AuthorizationCode,
    AuthorizationParams,
    AuthorizeError,
    RefreshToken,
    RegistrationError,
    TokenError,
)
from mcp.shared.auth import (
    InvalidRedirectUriError,
    InvalidScopeError,
    OAuthClientInformationFull,
)
from mcp.shared.auth import OAuthToken as OAuthTokenResponse
from pydantic import AnyUrl
from sqlalchemy import delete, or_, update

from mpri_mcp.config import Settings
from mpri_mcp.db import Database
from mpri_mcp.db.models import (
    OAuthAuthorizationCode,
    OAuthClient,
    OAuthPendingAuthorization,
    OAuthToken,
)
from mpri_mcp.utils.time import utcnow

log = logging.getLogger(__name__)

PENDING_TTL = timedelta(minutes=10)
_LOOPBACK_HOSTS = {"localhost", "127.0.0.1", "[::1]", "::1"}
# Accepted on requests but not required; refresh tokens are issued regardless.
OFFLINE_ACCESS = "offline_access"


def token_hash(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def _is_loopback(uri: str) -> bool:
    parsed = urlparse(uri)
    return parsed.scheme == "http" and (parsed.hostname or "") in _LOOPBACK_HOSTS


def redirect_uri_matches(registered: str, requested: str) -> bool:
    """Exact match, except loopback URIs match on any port (RFC 8252 §7.3) — Claude Code and
    MCP Inspector use an ephemeral port per session."""
    if registered == requested:
        return True
    if _is_loopback(registered) and _is_loopback(requested):
        a, b = urlparse(registered), urlparse(requested)
        return (a.hostname, a.path or "/", a.query) == (b.hostname, b.path or "/", b.query)
    return False


class MpriClient(OAuthClientInformationFull):
    """Registered client with port-agnostic loopback matching and lenient scope handling."""

    def validate_redirect_uri(self, redirect_uri: AnyUrl | None) -> AnyUrl:
        if redirect_uri is not None and self.redirect_uris:
            if any(redirect_uri_matches(str(r), str(redirect_uri)) for r in self.redirect_uris):
                return redirect_uri
            raise InvalidRedirectUriError(
                f"Redirect URI '{redirect_uri}' not registered for client"
            )
        return super().validate_redirect_uri(redirect_uri)

    def validate_scope(self, requested_scope: str | None) -> list[str] | None:
        if requested_scope is None:
            return None
        allowed = set((self.scope or "").split()) | {OFFLINE_ACCESS}
        requested = requested_scope.split()
        unknown = [s for s in requested if s not in allowed]
        if unknown:
            raise InvalidScopeError(f"Client was not registered with scope {' '.join(unknown)}")
        return requested


@dataclass
class PendingRequest:
    id: str
    client: MpriClient
    redirect_uri: str
    redirect_uri_provided_explicitly: bool
    state: str | None
    code_challenge: str
    scopes: list[str]
    resource: str | None


def _ts(dt: datetime) -> int:
    return int(dt.timestamp())


class SqliteOAuthProvider:
    def __init__(self, settings: Settings, db: Database) -> None:
        self._settings = settings
        self._db = db

    # ------------------------------------------------------------------ policy helpers
    def redirect_allowed(self, uri: str) -> bool:
        if uri in self._settings.oauth_allowed_redirect_uris:
            return True
        return self._settings.oauth_allow_loopback_redirects and _is_loopback(uri)

    def _resource_ok(self, resource: str | None) -> bool:
        if resource is None:
            return True
        return resource.rstrip("/") == self._settings.mcp_resource_url.rstrip("/")

    # ------------------------------------------------------------------ clients
    async def get_client(self, client_id: str) -> MpriClient | None:
        async with self._db.session() as s:
            row = await s.get(OAuthClient, client_id)
        if row is None or row.revoked_at is not None:
            return None
        return MpriClient.model_validate({**row.info, "client_secret": row.client_secret})

    async def register_client(self, client_info: OAuthClientInformationFull) -> None:
        uris = [str(u) for u in client_info.redirect_uris or []]
        rejected = [u for u in uris if not self.redirect_allowed(u)]
        if not uris or rejected:
            raise RegistrationError(
                error="invalid_redirect_uri",
                error_description=(
                    f"Redirect URI not allowed: {', '.join(rejected) or '(none)'}. "
                    "Allowed: Claude's callback and loopback addresses."
                ),
            )
        info = client_info.model_dump(mode="json", exclude={"client_secret"})
        async with self._db.session() as s:
            s.add(
                OAuthClient(
                    client_id=client_info.client_id,
                    client_secret=client_info.client_secret,
                    client_name=client_info.client_name,
                    info=info,
                )
            )
        log.info("OAuth client registered: %s (%s)", client_info.client_name, client_info.client_id)

    # ------------------------------------------------------------------ authorize
    async def authorize(
        self, client: OAuthClientInformationFull, params: AuthorizationParams
    ) -> str:
        if not self._resource_ok(params.resource):
            raise AuthorizeError(
                error="invalid_target",
                error_description=(
                    f"This server only issues tokens for {self._settings.mcp_resource_url}"
                ),
            )
        scopes = list(dict.fromkeys(params.scopes or [self._settings.oauth_scope]))
        if self._settings.oauth_scope not in scopes:
            scopes.insert(0, self._settings.oauth_scope)
        pending_id = secrets.token_urlsafe(24)
        now = utcnow()
        async with self._db.session() as s:
            s.add(
                OAuthPendingAuthorization(
                    id=pending_id,
                    client_id=client.client_id,
                    redirect_uri=str(params.redirect_uri),
                    redirect_uri_provided_explicitly=params.redirect_uri_provided_explicitly,
                    state=params.state,
                    code_challenge=params.code_challenge,
                    scopes=scopes,
                    resource=params.resource or self._settings.mcp_resource_url,
                    created_at=now,
                    expires_at=now + PENDING_TTL,
                )
            )
        return f"/oauth/consent?req={pending_id}"

    async def get_pending(self, pending_id: str) -> PendingRequest | None:
        async with self._db.session() as s:
            row = await s.get(OAuthPendingAuthorization, pending_id)
        if row is None or row.expires_at < utcnow():
            return None
        client = await self.get_client(row.client_id)
        if client is None:
            return None
        return PendingRequest(
            id=row.id,
            client=client,
            redirect_uri=row.redirect_uri,
            redirect_uri_provided_explicitly=row.redirect_uri_provided_explicitly,
            state=row.state,
            code_challenge=row.code_challenge,
            scopes=list(row.scopes),
            resource=row.resource,
        )

    async def complete_pending(
        self, pending: PendingRequest, *, approve: bool, subject: str
    ) -> str | None:
        """Consume the pending request. Returns a new authorization code when approved."""
        async with self._db.session() as s:
            await s.execute(
                delete(OAuthPendingAuthorization).where(OAuthPendingAuthorization.id == pending.id)
            )
            if not approve:
                return None
            code = secrets.token_urlsafe(32)
            now = utcnow()
            s.add(
                OAuthAuthorizationCode(
                    code_hash=token_hash(code),
                    client_id=pending.client.client_id,
                    redirect_uri=pending.redirect_uri,
                    redirect_uri_provided_explicitly=pending.redirect_uri_provided_explicitly,
                    code_challenge=pending.code_challenge,
                    scopes=pending.scopes,
                    resource=pending.resource,
                    subject=subject,
                    family_id=secrets.token_hex(16),
                    created_at=now,
                    expires_at=now + timedelta(seconds=self._settings.oauth_code_ttl_seconds),
                )
            )
        return code

    # ------------------------------------------------------------------ codes
    async def load_authorization_code(
        self, client: OAuthClientInformationFull, authorization_code: str
    ) -> AuthorizationCode | None:
        async with self._db.session() as s:
            row = await s.get(OAuthAuthorizationCode, token_hash(authorization_code))
            if row is None or row.client_id != client.client_id:
                return None
            if row.used_at is not None:
                # A replayed code means it leaked: kill everything issued from it.
                await self._revoke_family(s, row.family_id)
                log.warning("Authorization code replay for client %s", client.client_id)
                return None
            return AuthorizationCode(
                code=authorization_code,
                scopes=list(row.scopes),
                expires_at=row.expires_at.timestamp(),
                client_id=row.client_id,
                code_challenge=row.code_challenge,
                redirect_uri=AnyUrl(row.redirect_uri),
                redirect_uri_provided_explicitly=row.redirect_uri_provided_explicitly,
                resource=row.resource,
                subject=row.subject,
            )

    async def exchange_authorization_code(
        self, client: OAuthClientInformationFull, authorization_code: AuthorizationCode
    ) -> OAuthTokenResponse:
        async with self._db.session() as s:
            now = utcnow()
            claimed = await s.execute(
                update(OAuthAuthorizationCode)
                .where(
                    OAuthAuthorizationCode.code_hash == token_hash(authorization_code.code),
                    OAuthAuthorizationCode.used_at.is_(None),
                )
                .values(used_at=now)
            )
            if claimed.rowcount != 1:
                raise TokenError(
                    error="invalid_grant", error_description="authorization code already used"
                )
            row = await s.get(OAuthAuthorizationCode, token_hash(authorization_code.code))
            assert row is not None
            await self._touch_client(s, client.client_id, now)
            return self._issue(
                s,
                client_id=client.client_id,
                scopes=authorization_code.scopes,
                resource=authorization_code.resource,
                subject=authorization_code.subject,
                family_id=row.family_id,
            )

    # ------------------------------------------------------------------ tokens
    def _issue(
        self,
        s: Any,
        *,
        client_id: str,
        scopes: list[str],
        resource: str | None,
        subject: str | None,
        family_id: str,
    ) -> OAuthTokenResponse:
        access, refresh = (
            f"mpri_at_{secrets.token_urlsafe(32)}",
            f"mpri_rt_{secrets.token_urlsafe(32)}",
        )
        now = utcnow()
        common = dict(
            client_id=client_id,
            scopes=scopes,
            resource=resource,
            subject=subject,
            family_id=family_id,
            created_at=now,
        )
        access_ttl = self._settings.oauth_access_token_ttl_seconds
        s.add(
            OAuthToken(
                token_hash=token_hash(access),
                kind="access",
                expires_at=now + timedelta(seconds=access_ttl),
                **common,
            )
        )
        s.add(
            OAuthToken(
                token_hash=token_hash(refresh),
                kind="refresh",
                expires_at=now + timedelta(seconds=self._settings.oauth_refresh_token_ttl_seconds),
                **common,
            )
        )
        return OAuthTokenResponse(
            access_token=access,
            token_type="Bearer",
            expires_in=access_ttl,
            refresh_token=refresh,
            scope=" ".join(scopes),
        )

    async def load_refresh_token(
        self, client: OAuthClientInformationFull, refresh_token: str
    ) -> RefreshToken | None:
        async with self._db.session() as s:
            row = await s.get(OAuthToken, token_hash(refresh_token))
            if row is None or row.kind != "refresh" or row.client_id != client.client_id:
                return None
            if row.revoked_at is not None:
                return None
            if row.used_at is not None:
                # Rotated tokens must never come back; if one does, assume theft.
                await self._revoke_family(s, row.family_id)
                log.warning("Refresh token reuse for client %s; family revoked", client.client_id)
                return None
            return RefreshToken(
                token=refresh_token,
                client_id=row.client_id,
                scopes=list(row.scopes),
                expires_at=_ts(row.expires_at),
                resource=row.resource,
                subject=row.subject,
            )

    async def exchange_refresh_token(
        self,
        client: OAuthClientInformationFull,
        refresh_token: RefreshToken,
        scopes: list[str],
    ) -> OAuthTokenResponse:
        async with self._db.session() as s:
            now = utcnow()
            h = token_hash(refresh_token.token)
            claimed = await s.execute(
                update(OAuthToken)
                .where(
                    OAuthToken.token_hash == h,
                    OAuthToken.used_at.is_(None),
                    OAuthToken.revoked_at.is_(None),
                )
                .values(used_at=now)
            )
            if claimed.rowcount != 1:
                raise TokenError(
                    error="invalid_grant", error_description="refresh token already used"
                )
            row = await s.get(OAuthToken, h)
            assert row is not None
            await self._touch_client(s, client.client_id, now)
            return self._issue(
                s,
                client_id=client.client_id,
                scopes=scopes,
                resource=refresh_token.resource,
                subject=refresh_token.subject,
                family_id=row.family_id,
            )

    async def load_access_token(self, token: str) -> AccessToken | None:
        async with self._db.session() as s:
            row = await s.get(OAuthToken, token_hash(token))
            if row is None or row.kind != "access" or row.revoked_at is not None:
                return None
            now = utcnow()
            if row.expires_at < now:
                return None
            client = await s.get(OAuthClient, row.client_id)
            if client is None or client.revoked_at is not None:
                return None
            # Throttled bookkeeping for the dashboard's "last used".
            if row.last_used_at is None or now - row.last_used_at > timedelta(minutes=1):
                row.last_used_at = now
                client.last_used_at = now
            return AccessToken(
                token=token,
                client_id=row.client_id,
                scopes=list(row.scopes),
                expires_at=_ts(row.expires_at),
                resource=row.resource,
                subject=row.subject,
            )

    async def revoke_token(self, token: AccessToken | RefreshToken) -> None:
        async with self._db.session() as s:
            row = await s.get(OAuthToken, token_hash(token.token))
            if row is not None:
                await self._revoke_family(s, row.family_id)

    # ------------------------------------------------------------------ admin
    async def revoke_client(self, client_id: str) -> None:
        async with self._db.session() as s:
            row = await s.get(OAuthClient, client_id)
            if row is None:
                return
            now = utcnow()
            row.revoked_at = now
            await s.execute(
                update(OAuthToken)
                .where(OAuthToken.client_id == client_id, OAuthToken.revoked_at.is_(None))
                .values(revoked_at=now)
            )

    async def cleanup_expired(self) -> None:
        """Delete expired pending requests, codes and tokens (keeps the tables small)."""
        now = utcnow()
        cutoff = now - timedelta(days=1)
        async with self._db.session() as s:
            await s.execute(
                delete(OAuthPendingAuthorization).where(OAuthPendingAuthorization.expires_at < now)
            )
            await s.execute(
                delete(OAuthAuthorizationCode).where(OAuthAuthorizationCode.expires_at < cutoff)
            )
            await s.execute(
                delete(OAuthToken).where(
                    or_(OAuthToken.expires_at < cutoff, OAuthToken.revoked_at < cutoff)
                )
            )

    async def _revoke_family(self, s: Any, family_id: str) -> None:
        await s.execute(
            update(OAuthToken)
            .where(OAuthToken.family_id == family_id, OAuthToken.revoked_at.is_(None))
            .values(revoked_at=utcnow())
        )

    async def _touch_client(self, s: Any, client_id: str, now: datetime) -> None:
        row = await s.get(OAuthClient, client_id)
        if row is not None:
            row.last_used_at = now


class DbTokenVerifier:
    """TokenVerifier for MCPServer (the resource server): looks tokens up in SQLite."""

    def __init__(self, provider: SqliteOAuthProvider) -> None:
        self._provider = provider

    async def verify_token(self, token: str) -> AccessToken | None:
        return await self._provider.load_access_token(token)
