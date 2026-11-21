"""Dashboard: overview, crawl runs, connected OAuth apps."""

from __future__ import annotations

from fastapi import APIRouter, Depends, Request
from fastapi.responses import RedirectResponse
from starlette.responses import Response

from mpri_mcp.admin.web import flash, render, require_admin, state, verify_csrf
from mpri_mcp.services import agent_tasks, products, reviews, runs, supermarkets

router = APIRouter(dependencies=[Depends(require_admin)])


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
@router.get("/admin")
async def overview(request: Request) -> Response:
    st = state(request)
    await agent_tasks.maintain(st.db, st.settings)
    summaries = await supermarkets.list_supermarkets(st.db)
    clients = await st.oauth.list_clients()
    return render(
        request,
        "overview.html",
        summaries=summaries,
        names={s.supermarket.id: s.supermarket.name for s in summaries},
        product_count=await products.count_products(st.db),
        url_count=sum(s.active_url_count for s in summaries),
        failing=await runs.failing_urls(st.db, 10),
        recent=await runs.recent_runs(st.db, 8),
        pending_tasks=sum(s.pending_tasks for s in summaries),
        review=await reviews.list_needs_review(st.db, 50),
        review_count=await reviews.count_needs_review(st.db),
        scheduler=st.container.scheduler,
        connector_url=st.settings.mcp_resource_url,
        connected=[c for c in clients if not c.revoked_at and c.active_grants],
    )


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
@router.post("/admin/prices/{record_id}/accept")
async def price_accept(request: Request, record_id: int) -> Response:
    await verify_csrf(request)
    await reviews.accept_price(state(request).db, record_id)
    flash(request, "Price accepted.", "success")
    return RedirectResponse("/admin#review", status_code=303)


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
@router.post("/admin/prices/{record_id}/reject")
async def price_reject(request: Request, record_id: int) -> Response:
    await verify_csrf(request)
    await reviews.reject_price(state(request).db, record_id)
    flash(request, "Price rejected and deleted.", "success")
    return RedirectResponse("/admin#review", status_code=303)


@router.get("/admin/runs")
async def runs_list(request: Request, market: str | None = None) -> Response:
    st = state(request)
    summaries = await supermarkets.list_supermarkets(st.db)
    return render(
        request,
        "runs.html",
        runs=await runs.recent_runs(st.db, 100, market or None),
        names={s.supermarket.id: s.supermarket.name for s in summaries},
        market=market or "",
    )


@router.get("/admin/runs/{run_id}")
async def run_detail(request: Request, run_id: int) -> Response:
    st = state(request)
    run, items = await runs.get_run(st.db, run_id)
    market = await supermarkets.get_supermarket(st.db, run.supermarket_id)
    return render(request, "run_detail.html", run=run, items=items, market=market)


@router.get("/admin/apps")
async def apps(request: Request) -> Response:
    st = state(request)
    return render(
        request,
        "apps.html",
        clients=await st.oauth.list_clients(),
        connector_url=st.settings.mcp_resource_url,
    )


@router.post("/admin/apps/{client_id}/revoke")
async def revoke_app(request: Request, client_id: str) -> Response:
    await verify_csrf(request)
    await state(request).oauth.revoke_client(client_id)
    flash(request, "Access revoked. That app must be connected again to be used.", "success")
    return RedirectResponse("/admin/apps", status_code=303)
