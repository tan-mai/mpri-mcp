"""Turn raw page data into a compact, AI-friendly digest of the main product area."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any

from mpri_mcp.utils.text import normalize_name

# Headings after which pages list *other* products; text past them only confuses extraction.
RELATED_MARKERS = (
    "có thể bạn sẽ thích",
    "có thể bạn cũng thích",
    "sản phẩm tương tự",
    "sản phẩm liên quan",
    "sản phẩm đã xem",
    "gợi ý cho bạn",
    "thường được mua cùng",
    "khách hàng cũng mua",
    "you may also like",
    "related products",
    "similar products",
)
_PRODUCT_TYPES = {"Product", "ProductGroup", "IndividualProduct"}
_PRODUCT_KEYS = ("name", "sku", "gtin13", "gtin", "brand", "offers")
_OFFER_KEYS = (
    "price",
    "lowPrice",
    "highPrice",
    "priceCurrency",
    "availability",
    "priceValidUntil",
    "priceSpecification",
)


@dataclass
class PageDigest:
    url: str
    title: str
    h1: str
    products: list[dict[str, Any]] = field(default_factory=list)
    meta: dict[str, str] = field(default_factory=dict)
    hints: list[str] = field(default_factory=list)
    text: str = ""

    def to_prompt(self) -> str:
        parts = [
            f"URL: {self.url}",
            f"Page title: {self.title}",
            f"Main heading (H1): {self.h1 or '(none)'}",
        ]
        if self.products:
            parts.append(
                "Structured data (JSON-LD, may be incomplete):\n"
                + json.dumps(self.products, ensure_ascii=False)
            )
        if self.meta:
            parts.append("Meta tags:\n" + "\n".join(f"{k}: {v}" for k, v in self.meta.items()))
        if self.hints:
            parts.append("Price-related page elements (label: text):\n" + "\n".join(self.hints))
        parts.append("Visible text of the product area:\n" + (self.text or "(empty)"))
        return "\n\n".join(parts)


def _types(node: dict[str, Any]) -> set[str]:
    t = node.get("@type")
    return set(t) if isinstance(t, list) else {t} if isinstance(t, str) else set()


def _walk_jsonld(node: Any) -> list[dict[str, Any]]:
    if isinstance(node, list):
        return [n for item in node for n in _walk_jsonld(item)]
    if not isinstance(node, dict):
        return []
    found = [node] if _types(node) & _PRODUCT_TYPES else []
    if "@graph" in node:
        found += _walk_jsonld(node["@graph"])
    return found


def _trim_offers(offers: Any) -> Any:
    if isinstance(offers, list):
        return [_trim_offers(o) for o in offers[:5]]
    if isinstance(offers, dict):
        return {k: offers[k] for k in _OFFER_KEYS if k in offers}
    return offers


def jsonld_products(blocks: list[str]) -> list[dict[str, Any]]:
    products = []
    for block in blocks:
        try:
            data = json.loads(block)
        except (json.JSONDecodeError, TypeError):
            continue
        for node in _walk_jsonld(data):
            trimmed = {k: node[k] for k in _PRODUCT_KEYS if k in node}
            if isinstance(trimmed.get("brand"), dict):
                trimmed["brand"] = trimmed["brand"].get("name")
            if "offers" in trimmed:
                trimmed["offers"] = _trim_offers(trimmed["offers"])
            products.append(trimmed)
    return products[:3]


def _clean_lines(text: str) -> list[str]:
    lines: list[str] = []
    for raw in text.splitlines():
        line = " ".join(raw.split())
        if line and (not lines or lines[-1] != line):
            lines.append(line)
    return lines


def focus_text(text: str, h1: str, max_chars: int) -> str:
    """Start just before the product heading, stop at 'related products', cap the length."""
    lines = _clean_lines(text)
    start = 0
    target = normalize_name(h1) if h1 else ""
    if target:
        for i, line in enumerate(lines):
            if normalize_name(line) == target:
                start = max(0, i - 3)  # keep a little context (breadcrumb, badges)
                break
    end = len(lines)
    for i in range(start + 1, len(lines)):
        lowered = lines[i].lower()
        if any(marker in lowered for marker in RELATED_MARKERS):
            end = i
            break
    focused = "\n".join(lines[start:end])
    if len(focused) > max_chars:
        focused = focused[:max_chars].rsplit("\n", 1)[0] + "\n[…truncated]"
    return focused


def build_digest(url: str, raw: dict[str, Any], max_chars: int = 8000) -> PageDigest:
    hints = []
    for h in raw.get("hints") or []:
        text = h.get("text", "")
        if h.get("amount"):
            text = f"{text} (data-price-amount={h['amount']})"
        hints.append(f"{h.get('label') or 'price'}: {text}")
    return PageDigest(
        url=url,
        title=(raw.get("title") or "").strip(),
        h1=(raw.get("h1") or "").strip(),
        products=jsonld_products(raw.get("jsonld") or []),
        meta=dict(raw.get("meta") or {}),
        hints=hints,
        text=focus_text(raw.get("text") or "", raw.get("h1") or "", max_chars),
    )
