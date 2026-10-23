"""AI price extraction: prompt, schema and sanity checks — identical for every provider."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from pydantic import BaseModel, Field

from mpri_mcp.ai.base import StructuredResult
from mpri_mcp.ai.registry import AIChoice, ProviderRegistry
from mpri_mcp.crawler.digest import PageDigest
from mpri_mcp.i18n import N_

MAX_PLAUSIBLE_PRICE = 1_000_000_000  # 1 billion VND

SYSTEM_PROMPT = """\
You read product pages from Vietnamese supermarket websites and extract the price of the ONE \
main product on the page.

Rules:
- The main product is the one named in the page title / H1. Ignore prices of other products \
(recommendations, "Có thể bạn sẽ thích", "Sản phẩm tương tự", combos), shipping fees and \
vouchers.
- Return prices as integers in Vietnamese đồng (VND). Remove currency symbols and thousands \
separators, whichever separator the site uses: "65,000 ₫" -> 65000, "1.250.000đ" -> 1250000, \
"89k" -> 89000.
- current_price: what a regular customer pays now for one selling unit (the sale price when a \
promotion is running).
- original_price: the regular price shown crossed out or labelled as old/list price ("giá gốc", \
"giá cũ"). null when the page shows no discount.
- member_price: a lower price only for members or card holders ("Giá thành viên MCard", "giá \
hội viên", app-only price). null when there is none.
- Multi-buy offers ("Mua từ 3 hộp ...", "mua 2 tặng 1") are not the current_price.
- in_stock: false only if the page says the product is out of stock ("hết hàng", "tạm hết"); \
null if unknown.
- If the page is not a single product page (error page, search/category page, captcha, login \
wall) or shows no price, set is_product_page to false and the prices to null.
- Never guess. Use null for anything not on the page.
"""


class PriceExtraction(BaseModel):
    is_product_page: bool = Field(
        description="True only if the page shows one specific product with its price."
    )
    product_name: str | None = Field(description="Product name as shown on the page.")
    current_price: int | None = Field(
        description="Price a regular customer pays now for one selling unit, VND integer."
    )
    original_price: int | None = Field(
        description="Regular price before discount (crossed out), VND integer; null if no discount."
    )
    member_price: int | None = Field(
        description="Price for loyalty-card members only (e.g. MCard), VND integer; null if none."
    )
    in_stock: bool | None = Field(description="False if out of stock; null if unknown.")
    unit: str | None = Field(
        description="Selling unit shown with the price, e.g. 'Thùng', 'Hộp', 'kg'."
    )


@dataclass
class CleanPrice:
    current_price: int
    original_price: int
    member_price: int | None
    in_stock: bool | None
    notes: list[str]


class ExtractionRejected(Exception):
    """The AI answered, but the answer is not a usable price."""

    def __init__(self, reason: str, result: StructuredResult[PriceExtraction]) -> None:
        super().__init__(reason)
        self.result = result


@dataclass
class ExtractionOutcome:
    price: CleanPrice
    result: StructuredResult[PriceExtraction]

    def raw(self) -> dict[str, Any]:
        return {**self.result.data.model_dump(), "notes": self.price.notes}


def validate_extraction(data: PriceExtraction) -> CleanPrice:
    """Raises ValueError with a human-readable reason when the extraction is unusable."""
    if not data.is_product_page:
        raise ValueError(N_("Not a product page (or no price shown)"))
    current = data.current_price
    if current is None or current <= 0:
        raise ValueError(N_("No current price found on the page"))
    if current > MAX_PLAUSIBLE_PRICE:
        raise ValueError(f"Implausible price {current}")
    notes: list[str] = []
    original = data.original_price
    if original is None or original <= 0:
        original = current
    elif original < current:
        notes.append(f"original_price {original} < current_price; ignored")
        original = current
    member = data.member_price
    if member is not None and (member <= 0 or member >= current):
        notes.append(f"member_price {member} not below current_price; ignored")
        member = None
    return CleanPrice(current, original, member, data.in_stock, notes)


class PriceExtractor:
    def __init__(self, registry: ProviderRegistry) -> None:
        self._registry = registry

    async def extract(
        self, digest: PageDigest, choice: AIChoice, fallback: AIChoice | None = None
    ) -> ExtractionOutcome:
        result = await self._registry.generate_structured(
            system=SYSTEM_PROMPT,
            prompt=digest.to_prompt(),
            schema=PriceExtraction,
            choice=choice,
            fallback=fallback,
        )
        try:
            price = validate_extraction(result.data)
        except ValueError as e:
            raise ExtractionRejected(str(e), result) from e
        return ExtractionOutcome(price=price, result=result)
