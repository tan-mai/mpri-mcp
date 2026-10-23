"""Dashboard: products, their URLs, price history and single-URL re-tests."""

from __future__ import annotations

from fastapi import APIRouter, Depends, Request
from fastapi.responses import JSONResponse, RedirectResponse
from starlette.responses import Response

from mpri_mcp.admin.web import flash, form_data, render, require_admin, state, verify_csrf
from mpri_mcp.i18n import _
from mpri_mcp.services import analytics, products, supermarkets
from mpri_mcp.services.errors import ServiceError
from mpri_mcp.services.products import UrlInput

router = APIRouter(dependencies=[Depends(require_admin)])


def _detail(product_id: int) -> RedirectResponse:
    return RedirectResponse(f"/admin/products/{product_id}", status_code=303)


async def _list_page(request: Request, q: str = "", status_code: int = 200, **extra) -> Response:
    st = state(request)
    return render(
        request,
        "products.html",
        status_code=status_code,
        items=await products.list_products(st.db, q or None),
        markets=[s.supermarket for s in await supermarkets.list_supermarkets(st.db)],
        q=q,
        form=extra.pop("form", {}),
        **extra,
    )


@router.get("/admin/products")
async def product_list(request: Request, q: str = "") -> Response:
    return await _list_page(request, q)


@router.post("/admin/products")
async def product_add(request: Request) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    urls = [
        UrlInput(key.removeprefix("url__"), value.strip())
        for key, value in form.items()
        if key.startswith("url__") and value.strip()
    ]
    try:
        result = await products.add_product_with_urls(
            st.db, form.get("name", ""), form.get("category"), urls
        )
    except ServiceError as e:
        return await _list_page(request, status_code=400, form=form, error=str(e))
    for r in result.urls:
        if r.status == "error":
            flash(request, f"{r.supermarket_id}: {r.message}", "error")
        elif r.warning:
            flash(request, f"{r.supermarket_id}: {r.warning}", "warning")
    message = _("Added '{name}'.") if result.created else _("Updated '{name}'.")
    flash(request, message.format(name=result.product.name), "success")
    return _detail(result.product.id)


@router.get("/admin/products/{product_id}")
async def product_detail(request: Request, product_id: int) -> Response:
    st = state(request)
    product = await products.get_product(st.db, product_id)
    prices = await analytics.latest_prices(st.db, product)
    tracked = {u.supermarket_id for u in product.urls}
    markets = [s.supermarket for s in await supermarkets.list_supermarkets(st.db)]
    return render(
        request,
        "product_detail.html",
        product=product,
        prices=prices,
        urls=sorted(product.urls, key=lambda u: u.supermarket.name),
        untracked=[m for m in markets if m.id not in tracked],
    )


@router.post("/admin/products/{product_id}")
async def product_edit(request: Request, product_id: int) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    try:
        await products.update_product(
            state(request).db, product_id, name=form.get("name", ""), category=form.get("category")
        )
        flash(request, _("Saved."), "success")
    except ServiceError as e:
        flash(request, str(e), "error")
    return _detail(product_id)


@router.post("/admin/products/{product_id}/delete")
async def product_delete(request: Request, product_id: int) -> Response:
    await verify_csrf(request)
    await products.delete_product(state(request).db, product_id)
    flash(request, _("Product deleted with its price history."), "success")
    return RedirectResponse("/admin/products", status_code=303)


@router.post("/admin/products/{product_id}/urls")
async def product_add_url(request: Request, product_id: int) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    product = await products.get_product(st.db, product_id)
    try:
        result = await products.add_product_with_urls(
            st.db,
            product.name,
            None,
            [UrlInput(form.get("supermarket_id", ""), form.get("url", ""))],
        )
        r = result.urls[0]
        if r.status == "error":
            flash(request, r.message or _("Could not add URL."), "error")
        else:
            flash(request, r.warning or _("URL added."), "warning" if r.warning else "success")
    except ServiceError as e:
        flash(request, str(e), "error")
    return _detail(product_id)


@router.post("/admin/urls/{url_id}")
async def url_edit(request: Request, url_id: int) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    row = await products.get_url(st.db, url_id)
    try:
        await products.update_url(
            st.db,
            url_id,
            url=form.get("url") or None,
            is_active=form.get("is_active") == "on",
        )
        flash(request, _("URL saved."), "success")
    except ServiceError as e:
        flash(request, str(e), "error")
    return _detail(row.product_id)


@router.post("/admin/urls/{url_id}/delete")
async def url_delete(request: Request, url_id: int) -> Response:
    await verify_csrf(request)
    st = state(request)
    row = await products.get_url(st.db, url_id)
    await products.delete_url(st.db, url_id)
    flash(
        request,
        _("Stopped tracking at {name}.").format(name=row.supermarket.name),
        "success",
    )
    return _detail(row.product_id)


@router.post("/admin/urls/{url_id}/test")
async def url_test(request: Request, url_id: int) -> Response:
    """htmx fragment: fetch + extract this URL now without saving."""
    await verify_csrf(request)
    st = state(request)
    row = await products.get_url(st.db, url_id)
    result = await st.container.runner.dry_run(row.url, supermarket_id=row.supermarket_id)
    return render(request, "_dry_run.html", result=result)


@router.get("/admin/products/{product_id}/history.json")
async def product_history(request: Request, product_id: int, days: int = 90) -> JSONResponse:
    st = state(request)
    product = await products.get_product(st.db, product_id)
    history = await analytics.product_history(
        st.db, product, max(1, min(days, 730)), tz=st.settings.scheduler_timezone
    )
    return JSONResponse(
        {
            "series": [
                {
                    "name": s.supermarket_name,
                    "points": [
                        {
                            "date": p.day.isoformat(),
                            "current": p.current_price,
                            "original": p.original_price,
                            "member": p.member_price,
                        }
                        for p in s.points
                    ],
                }
                for s in history.series
            ]
        }
    )
