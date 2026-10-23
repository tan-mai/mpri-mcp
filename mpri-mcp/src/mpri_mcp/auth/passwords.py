"""Password hashing (argon2id) and validation rules."""

from __future__ import annotations

import re

from argon2 import PasswordHasher
from argon2.exceptions import InvalidHashError, VerificationError

from mpri_mcp.i18n import _

_hasher = PasswordHasher()
# Pre-computed so a login for an unknown username costs the same as a real verify.
_DUMMY_HASH = _hasher.hash("dummy-password-for-timing")

MIN_PASSWORD_LENGTH = 12
_USERNAME_RE = re.compile(r"^[A-Za-z0-9._-]{3,50}$")


def hash_password(password: str) -> str:
    return _hasher.hash(password)


def verify_password(password_hash: str | None, password: str) -> bool:
    try:
        return _hasher.verify(password_hash or _DUMMY_HASH, password) and password_hash is not None
    except (VerificationError, InvalidHashError):
        return False


def needs_rehash(password_hash: str) -> bool:
    return _hasher.check_needs_rehash(password_hash)


def validate_username(username: str) -> str | None:
    if not _USERNAME_RE.match(username):
        return _("Username must be 3–50 characters: letters, digits, dot, dash or underscore.")
    return None


def validate_new_password(password: str, confirm: str, username: str) -> str | None:
    if len(password) < MIN_PASSWORD_LENGTH:
        return _("Password must be at least {n} characters.").format(n=MIN_PASSWORD_LENGTH)
    if password != confirm:
        return _("Passwords do not match.")
    if password.lower() == username.lower():
        return _("Password must not be the same as the username.")
    return None
