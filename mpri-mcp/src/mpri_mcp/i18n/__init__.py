"""Translations (English source strings, Vietnamese catalog) and locale-aware formatting.

The current language lives in a context variable: the dashboard sets it per request from the
language cookie, MCP tools set it per call from the connection's saved preference.

    _("Saved.")                                   # translate now
    N_("Supermarket '{id}' not found.")           # mark for extraction, translate later
    _("Added {name}.").format(name=market.name)   # Python strings use str.format placeholders
    {{ _("Signed in as %(name)s.", name=x) }}     # templates use %(name)s placeholders
"""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar, Token
from datetime import datetime, timedelta
from functools import cache
from pathlib import Path
from zoneinfo import ZoneInfo

from babel.dates import format_datetime as _babel_datetime
from babel.dates import format_timedelta
from babel.messages.pofile import read_po

from mpri_mcp.utils.time import utcnow

LANGUAGES: dict[str, str] = {"en": "English", "vi": "Tiếng Việt"}
SOURCE_LANGUAGE = "en"
LOCALES_DIR = Path(__file__).parent / "locales"

_current: ContextVar[str | None] = ContextVar("mpri_language", default=None)
_default = SOURCE_LANGUAGE


def normalize_language(value: str | None) -> str | None:
    """'vi', 'VI', 'vi-VN' -> 'vi'; unknown -> None."""
    if not value:
        return None
    code = value.strip().lower().replace("_", "-").split("-")[0]
    return code if code in LANGUAGES else None


def set_default_language(language: str) -> None:
    global _default
    _default = normalize_language(language) or SOURCE_LANGUAGE


def default_language() -> str:
    return _default


def get_language() -> str:
    return _current.get() or _default


def activate(language: str | None) -> Token[str | None]:
    return _current.set(normalize_language(language))


def deactivate(token: Token[str | None]) -> None:
    _current.reset(token)


@contextmanager
def use_language(language: str | None) -> Iterator[None]:
    token = activate(language)
    try:
        yield
    finally:
        deactivate(token)


@cache
def _catalog(language: str) -> dict[str, str]:
    path = LOCALES_DIR / language / "LC_MESSAGES" / "messages.po"
    if not path.exists():
        return {}
    with path.open("rb") as f:
        catalog = read_po(f, locale=language)
    return {
        str(m.id): str(m.string)
        for m in catalog
        if m.id and isinstance(m.id, str) and m.string and not m.fuzzy
    }


def gettext(message: str) -> str:
    language = get_language()
    if language == SOURCE_LANGUAGE:
        return message
    return _catalog(language).get(message) or message


def ngettext(singular: str, plural: str, n: int) -> str:
    # Only needed by the Jinja i18n extension; our strings avoid plural forms
    # (Vietnamese has none, and English wording is chosen to not need them).
    return gettext(singular if n == 1 else plural)


def translate_stored(text: str | None) -> str | None:
    """Translate a message that was saved to the database in English (e.g. a crawl error).
    Fixed messages have catalog entries; dynamic ones ('HTTP 404') come back unchanged."""
    return gettext(text) if text else text


def N_(message: str) -> str:
    """Mark a string for extraction without translating it yet."""
    return message


_ = gettext


# --------------------------------------------------------------------------- formatting
_DATETIME_PATTERNS = {"en": "EEE dd/MM/yyyy HH:mm", "vi": "EEEE, dd/MM/yyyy HH:mm"}


def format_weekday_datetime(value: datetime | None, timezone: str) -> str | None:
    """'Sat 24/10/2026 03:00' / 'Thứ Bảy, 24/10/2026 03:00'."""
    if value is None:
        return None
    language = get_language()
    return _babel_datetime(
        value.astimezone(ZoneInfo(timezone)),
        _DATETIME_PATTERNS.get(language, _DATETIME_PATTERNS["en"]),
        locale=language,
    )


def format_relative(value: datetime | None, now: datetime | None = None) -> str:
    """'2 hours ago' / '2 giờ trước'; 'just now' under a minute; 'never' for None."""
    if value is None:
        return gettext("never")
    delta = value - (now or utcnow())
    if abs(delta) < timedelta(minutes=1):
        return gettext("just now")
    return format_timedelta(delta, add_direction=True, locale=get_language())
