"""Products and their per-supermarket URLs."""

from __future__ import annotations

from dataclasses import dataclass, field

from sqlalchemy import func, select
from sqlalchemy.orm import selectinload

from mpri_mcp.db import Database
from mpri_mcp.db.models import Product, ProductUrl, Supermarket
from mpri_mcp.services.errors import ConflictError, NotFoundError, ValidationError
from mpri_mcp.utils.text import FuzzyMatch, fuzzy_rank, normalize_name, same_site, validate_page_url


@dataclass
class UrlInput:
    supermarket_id: str
    url: str


@dataclass
class UrlResult:
    supermarket_id: str
    url: str
    status: str  # added | updated | unchanged | error
    message: str | None = None
    warning: str | None = None


@dataclass
class AddProductResult:
    product: Product
    created: bool
    urls: list[UrlResult] = field(default_factory=list)


@dataclass
class ProductListItem:
    product: Product
    url_count: int


def _clean_name(name: str) -> tuple[str, str]:
    name = " ".join(name.split())
    if not name:
        raise ValidationError("Product name is required.")
    if len(name) > 300:
        raise ValidationError("Product name is too long (max 300 characters).")
    normalized = normalize_name(name)
    if not normalized:
        raise ValidationError("Product name must contain letters or digits.")
    return name, normalized


async def add_product_with_urls(
    db: Database, name: str, category: str | None, urls: list[UrlInput]
) -> AddProductResult:
    name, normalized = _clean_name(name)
    category = (category or "").strip() or None

    async with db.session() as s:
        product = await s.scalar(select(Product).where(Product.normalized_name == normalized))
        created = product is None
        if product is None:
            product = Product(name=name, normalized_name=normalized, category=category)
            s.add(product)
            await s.flush()
        elif category and not product.category:
            product.category = category

        markets = {m.id: m for m in (await s.scalars(select(Supermarket))).all()}
        existing = {
            u.supermarket_id: u
            for u in (
                await s.scalars(select(ProductUrl).where(ProductUrl.product_id == product.id))
            ).all()
        }

        results: list[UrlResult] = []
        for item in urls:
            market = markets.get(item.supermarket_id)
            if market is None:
                results.append(
                    UrlResult(
                        item.supermarket_id,
                        item.url,
                        "error",
                        f"Unknown supermarket_id '{item.supermarket_id}'.",
                    )
                )
                continue
            try:
                url = validate_page_url(item.url)
            except ValueError as e:
                results.append(UrlResult(item.supermarket_id, item.url, "error", str(e)))
                continue
            warning = (
                None
                if same_site(url, market.base_url)
                else f"URL host does not match {market.name} ({market.base_url})."
            )
            row = existing.get(market.id)
            if row is None:
                row = ProductUrl(product_id=product.id, supermarket_id=market.id, url=url)
                s.add(row)
                existing[market.id] = row
                results.append(UrlResult(market.id, url, "added", warning=warning))
            elif row.url == url and row.is_active:
                results.append(UrlResult(market.id, url, "unchanged", warning=warning))
            else:
                row.url = url
                row.is_active = True
                row.consecutive_failures = 0
                row.last_error = None
                results.append(UrlResult(market.id, url, "updated", warning=warning))

    return AddProductResult(product=product, created=created, urls=results)


async def find_products(db: Database, query: str, limit: int = 5) -> list[FuzzyMatch[int]]:
    async with db.session() as s:
        rows = (await s.execute(select(Product.id, Product.name))).all()
    return fuzzy_rank(query, [(r.id, r.name) for r in rows], limit=limit)


async def resolve_product(db: Database, query: str) -> tuple[Product, list[FuzzyMatch[int]]]:
    """Best fuzzy match plus close alternatives (within 5 points) the caller may mention."""
    matches = await find_products(db, query, limit=5)
    if not matches:
        raise NotFoundError(
            f"No tracked product matches '{query}'. Add it first with add_product_with_urls."
        )
    best = matches[0]
    alternatives = [m for m in matches[1:] if best.score - m.score <= 5]
    product = await get_product(db, best.key)
    return product, alternatives


async def get_product(db: Database, product_id: int) -> Product:
    async with db.session() as s:
        product = await s.scalar(
            select(Product)
            .where(Product.id == product_id)
            .options(selectinload(Product.urls).selectinload(ProductUrl.supermarket))
        )
    if product is None:
        raise NotFoundError(f"Product #{product_id} not found.")
    return product


async def list_products(db: Database, query: str | None = None) -> list[ProductListItem]:
    async with db.session() as s:
        rows = (
            await s.execute(
                select(Product, func.count(ProductUrl.id))
                .outerjoin(ProductUrl, ProductUrl.product_id == Product.id)
                .group_by(Product.id)
                .order_by(Product.name)
            )
        ).all()
    items = [ProductListItem(product=p, url_count=c) for p, c in rows]
    if query and query.strip():
        ranked = fuzzy_rank(query, [(i.product.id, i.product.name) for i in items], limit=200)
        order = {m.key: n for n, m in enumerate(ranked)}
        items = sorted(
            (i for i in items if i.product.id in order), key=lambda i: order[i.product.id]
        )
    return items


async def count_products(db: Database) -> int:
    async with db.session() as s:
        return await s.scalar(select(func.count()).select_from(Product)) or 0


async def update_product(
    db: Database, product_id: int, *, name: str, category: str | None
) -> Product:
    name, normalized = _clean_name(name)
    async with db.session() as s:
        product = await s.get(Product, product_id)
        if product is None:
            raise NotFoundError(f"Product #{product_id} not found.")
        clash = await s.scalar(
            select(Product).where(Product.normalized_name == normalized, Product.id != product_id)
        )
        if clash:
            raise ConflictError(f"Another product is already named '{clash.name}'.")
        product.name, product.normalized_name = name, normalized
        product.category = (category or "").strip() or None
    return product


async def delete_product(db: Database, product_id: int) -> None:
    async with db.session() as s:
        product = await s.get(Product, product_id)
        if product is None:
            raise NotFoundError(f"Product #{product_id} not found.")
        await s.delete(product)


async def get_url(db: Database, url_id: int) -> ProductUrl:
    async with db.session() as s:
        row = await s.scalar(
            select(ProductUrl)
            .where(ProductUrl.id == url_id)
            .options(selectinload(ProductUrl.product), selectinload(ProductUrl.supermarket))
        )
    if row is None:
        raise NotFoundError(f"URL #{url_id} not found.")
    return row


async def update_url(
    db: Database, url_id: int, *, url: str | None = None, is_active: bool | None = None
) -> ProductUrl:
    async with db.session() as s:
        row = await s.get(ProductUrl, url_id)
        if row is None:
            raise NotFoundError(f"URL #{url_id} not found.")
        if url is not None:
            try:
                new_url = validate_page_url(url)
            except ValueError as e:
                raise ValidationError(str(e)) from e
            if new_url != row.url:
                row.url = new_url
                row.consecutive_failures = 0
                row.last_error = None
        if is_active is not None:
            row.is_active = is_active
    return row


async def delete_url(db: Database, url_id: int) -> None:
    async with db.session() as s:
        row = await s.get(ProductUrl, url_id)
        if row is None:
            raise NotFoundError(f"URL #{url_id} not found.")
        await s.delete(row)
