"""The single admin account: first-run setup token, creation, login checks, password changes."""

from __future__ import annotations

import hmac
import logging
import secrets
import time
from collections import defaultdict, deque

from sqlalchemy import func, select

from mpri_mcp.auth.passwords import hash_password, needs_rehash, verify_password
from mpri_mcp.config import Settings
from mpri_mcp.db import Database
from mpri_mcp.db.models import AdminUser
from mpri_mcp.utils.time import utcnow

log = logging.getLogger(__name__)


class AdminExistsError(Exception):
    pass


class SetupTokenManager:
    """One-time token that must be presented to create the admin account.

    Without it, whoever reached a fresh deployment first could claim it. The token is
    printed to the logs at startup (or comes from ADMIN_SETUP_TOKEN)."""

    def __init__(self, settings: Settings) -> None:
        self._settings = settings
        self._token: str | None = None

    @property
    def active(self) -> bool:
        return self._token is not None

    def issue(self) -> None:
        fixed = self._settings.admin_setup_token
        self._token = fixed.get_secret_value() if fixed else secrets.token_urlsafe(18)
        url = f"{self._settings.public_base_url}/admin/setup"
        shown = "(the value of ADMIN_SETUP_TOKEN)" if fixed else self._token
        banner = "=" * 70
        log.warning(
            "\n%s\n FIRST-RUN SETUP - no admin account exists yet.\n Open %s\n Setup token: %s\n%s",
            banner,
            url,
            shown,
            banner,
        )

    def verify(self, candidate: str) -> bool:
        if self._token is None or not candidate:
            return False
        return hmac.compare_digest(self._token.encode(), candidate.strip().encode())

    def clear(self) -> None:
        self._token = None


class LoginRateLimiter:
    """In-memory sliding window of failures per key (IP, username). One process, so this is
    authoritative; it resets on restart, which is acceptable for this threat model."""

    def __init__(self, max_failures: int = 5, window_seconds: float = 900) -> None:
        self.max_failures = max_failures
        self.window = window_seconds
        self._failures: dict[str, deque[float]] = defaultdict(deque)

    def _prune(self, key: str, now: float) -> deque[float]:
        q = self._failures[key]
        while q and now - q[0] > self.window:
            q.popleft()
        return q

    def blocked(self, *keys: str) -> bool:
        now = time.monotonic()
        return any(len(self._prune(k, now)) >= self.max_failures for k in keys)

    def record_failure(self, *keys: str) -> None:
        now = time.monotonic()
        for k in keys:
            self._prune(k, now).append(now)

    def reset(self, *keys: str) -> None:
        for k in keys:
            self._failures.pop(k, None)


async def admin_count(db: Database) -> int:
    async with db.session() as s:
        return await s.scalar(select(func.count()).select_from(AdminUser)) or 0


async def create_admin(db: Database, username: str, password: str) -> AdminUser:
    async with db.session() as s:
        if await s.scalar(select(func.count()).select_from(AdminUser)):
            raise AdminExistsError("An admin account already exists.")
        user = AdminUser(username=username, password_hash=hash_password(password))
        s.add(user)
    log.info("Admin account '%s' created", username)
    return user


async def authenticate(db: Database, username: str, password: str) -> AdminUser | None:
    async with db.session() as s:
        user = await s.scalar(select(AdminUser).where(AdminUser.username == username))
        if not verify_password(user.password_hash if user else None, password):
            return None
        assert user is not None
        if needs_rehash(user.password_hash):
            user.password_hash = hash_password(password)
        user.last_login_at = utcnow()
    return user


async def get_admin(db: Database, user_id: int) -> AdminUser | None:
    async with db.session() as s:
        return await s.get(AdminUser, user_id)


async def set_password(
    db: Database, *, username: str | None, password: str, create_if_missing: bool = False
) -> AdminUser:
    """Set a new password and invalidate every existing session (used by the CLI and the UI)."""
    async with db.session() as s:
        stmt = select(AdminUser)
        if username:
            stmt = stmt.where(AdminUser.username == username)
        user = await s.scalar(stmt.order_by(AdminUser.id))
        if user is None:
            if not (create_if_missing and username):
                raise LookupError("No matching admin account.")
            user = AdminUser(username=username, password_hash=hash_password(password))
            s.add(user)
            return user
        user.password_hash = hash_password(password)
        user.password_changed_at = utcnow()
        user.session_version += 1
    return user


async def bump_session_version(db: Database, user_id: int) -> AdminUser:
    async with db.session() as s:
        user = await s.get(AdminUser, user_id)
        assert user is not None
        user.session_version += 1
    return user
