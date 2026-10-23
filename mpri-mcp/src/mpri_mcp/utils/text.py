"""Vietnamese-aware text normalization and fuzzy matching."""

from __future__ import annotations

import re
import unicodedata
from collections.abc import Iterable
from dataclasses import dataclass
from urllib.parse import urlparse

from rapidfuzz import fuzz

from mpri_mcp.i18n import _

_NON_ALNUM = re.compile(r"[^a-z0-9]+")


def strip_diacritics(text: str) -> str:
    # NFD splits most Vietnamese letters into base + combining marks, but not đ/Đ.
    text = text.replace("đ", "d").replace("Đ", "D")
    decomposed = unicodedata.normalize("NFD", text)
    return "".join(ch for ch in decomposed if unicodedata.category(ch) != "Mn")


def normalize_name(text: str) -> str:
    """Lowercase, diacritic-free, single-spaced: 'Sữa TH Ít Đường' -> 'sua th it duong'."""
    return _NON_ALNUM.sub(" ", strip_diacritics(text).lower()).strip()


def slugify(text: str) -> str:
    return normalize_name(text).replace(" ", "-")


def normalize_base_url(url: str) -> str:
    """'lottemart.vn' -> 'https://lottemart.vn'. Raises ValueError for anything unusable."""
    url = url.strip()
    if not url:
        raise ValueError(_("URL is empty."))
    if "://" not in url:
        url = f"https://{url}"
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.netloc or "." not in parsed.netloc:
        raise ValueError(_("'{url}' is not a valid http(s) URL.").format(url=url))
    return f"{parsed.scheme}://{parsed.netloc.lower()}{parsed.path.rstrip('/')}"


def validate_page_url(url: str) -> str:
    url = url.strip()
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ValueError(_("'{url}' is not a valid http(s) URL.").format(url=url))
    return url


def registrable_host(url: str) -> str:
    """Host without a leading 'www.' — used to warn when a URL doesn't belong to a market."""
    host = urlparse(url if "://" in url else f"https://{url}").netloc.lower().split(":")[0]
    return host.removeprefix("www.")


def same_site(url: str, base_url: str) -> bool:
    host, base = registrable_host(url), registrable_host(base_url)
    return host == base or host.endswith(f".{base}") or base.endswith(f".{host}")


@dataclass(frozen=True)
class FuzzyMatch[K]:
    key: K
    name: str
    score: float


def fuzzy_rank[K](
    query: str,
    candidates: Iterable[tuple[K, str]],
    *,
    limit: int = 5,
    min_score: float = 60,
) -> list[FuzzyMatch[K]]:
    """Rank (key, display_name) candidates against a free-text query.

    token_set_ratio rewards queries that are a subset of the name ("satori 500ml"),
    token_sort_ratio breaks ties in favour of names that don't carry many extra words.
    """
    q = normalize_name(query)
    if not q:
        return []
    scored: list[tuple[float, float, FuzzyMatch[K]]] = []
    for key, name in candidates:
        n = normalize_name(name)
        primary = 100.0 if n == q else fuzz.token_set_ratio(q, n)
        if primary < min_score:
            continue
        tiebreak = fuzz.token_sort_ratio(q, n)
        scored.append((primary, tiebreak, FuzzyMatch(key=key, name=name, score=round(primary, 1))))
    scored.sort(key=lambda t: (t[0], t[1]), reverse=True)
    return [m for _, _, m in scored[:limit]]
