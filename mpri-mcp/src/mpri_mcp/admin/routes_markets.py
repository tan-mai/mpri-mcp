"""Dashboard: supermarkets, their schedules, crawl config and AI override."""

from __future__ import annotations

import json

from fastapi import APIRouter, Depends, Request
from fastapi.responses import RedirectResponse
from starlette.responses import Response

from mpri_mcp.admin.web import flash, form_data, render, require_admin, state, verify_csrf
from mpri_mcp.scheduler.cron import describe_times
from mpri_mcp.services import runs, schedules, supermarkets
from mpri_mcp.services.errors import ServiceError

router = APIRouter(dependencies=[Depends(require_admin)])


def _detail(market_id: str) -> RedirectResponse:
    return RedirectResponse(f"/admin/supermarkets/{market_id}", status_code=303)


@router.get("/admin/supermarkets")
async def market_list(request: Request) -> Response:
    st = state(request)
    return render(
        request,
        "supermarkets.html",
        summaries=await supermarkets.list_supermarkets(st.db),
        scheduler=st.container.scheduler,
        form={},
    )


@router.post("/admin/supermarkets")
async def market_add(request: Request) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    try:
        market = await supermarkets.add_supermarket(
            st.db, form.get("name", ""), form.get("base_url", "")
        )
    except ServiceError as e:
        return render(
            request,
            "supermarkets.html",
            status_code=400,
            summaries=await supermarkets.list_supermarkets(st.db),
            scheduler=st.container.scheduler,
            form=form,
            error=str(e),
        )
    flash(request, f"Added {market.name} (ID {market.id}). Set a crawl schedule below.", "success")
    return _detail(market.id)


@router.get("/admin/supermarkets/{market_id}")
async def market_detail(request: Request, market_id: str) -> Response:
    st = state(request)
    market = await supermarkets.get_supermarket(st.db, market_id)
    config_json = json.dumps(market.crawl_config or {}, ensure_ascii=False, indent=2)
    return render(
        request,
        "supermarket_detail.html",
        market=market,
        urls=sorted(await supermarkets.market_urls(st.db, market_id), key=lambda u: u.product.name),
        recent=await runs.recent_runs(st.db, 10, market_id),
        next_run=st.container.scheduler.next_run_time(market_id),
        running=st.container.scheduler.is_running(market_id),
        config_json=config_json,
        config_example=json.dumps(
            {
                "wait_for_selector": ".price",
                "text_selector": "main",
                "cookies": [{"name": "store_id", "value": "10010"}],
                "setup_actions": [{"action": "click", "selector": "#choose-store"}],
            },
            indent=2,
        ),
        providers=st.container.registry.statuses(),
    )


@router.post("/admin/supermarkets/{market_id}")
async def market_edit(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    try:
        await supermarkets.update_supermarket(
            state(request).db,
            market_id,
            name=form.get("name", ""),
            base_url=form.get("base_url", ""),
        )
        flash(request, "Saved.", "success")
    except ServiceError as e:
        flash(request, str(e), "error")
    return _detail(market_id)


@router.post("/admin/supermarkets/{market_id}/schedule")
async def market_schedule(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    try:
        if form.get("action") == "disable":
            await schedules.disable_cron(st.db, st.container.scheduler, market_id)
            flash(request, "Schedule paused.", "success")
        else:
            result = await schedules.set_cron(
                st.db, st.container.scheduler, market_id, form.get("cron_expression", "")
            )
            first = describe_times(result.next_runs[:1], result.timezone)
            flash(
                request,
                f"Schedule set to '{result.cron_expression}'. Next run: "
                f"{first[0] if first else 'never'}.",
                "success",
            )
    except ServiceError as e:
        flash(request, str(e), "error")
    return _detail(market_id)


@router.post("/admin/supermarkets/{market_id}/run")
async def market_run(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    st = state(request)
    try:
        started = await schedules.trigger_now(st.db, st.container.scheduler, market_id)
        flash(
            request,
            "Crawl started in the background — refresh in a minute."
            if started
            else "A crawl of this supermarket is already running.",
            "success" if started else "warning",
        )
    except ServiceError as e:
        flash(request, str(e), "error")
    return _detail(market_id)


@router.post("/admin/supermarkets/{market_id}/crawl-config")
async def market_crawl_config(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    raw = form.get("crawl_config", "").strip() or "{}"
    try:
        parsed = json.loads(raw)
        if not isinstance(parsed, dict):
            raise ValueError("crawl_config must be a JSON object")
        await supermarkets.update_supermarket(state(request).db, market_id, crawl_config=parsed)
        flash(request, "Crawl settings saved.", "success")
    except (ValueError, ServiceError) as e:
        flash(request, f"Not saved: {e}", "error")
    return _detail(market_id)


@router.post("/admin/supermarkets/{market_id}/ai")
async def market_ai(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    provider = form.get("ai_provider", "").strip()
    if provider and provider not in state(request).container.registry.names():
        flash(request, f"Unknown AI provider '{provider}'.", "error")
        return _detail(market_id)
    try:
        await supermarkets.update_supermarket(
            state(request).db,
            market_id,
            ai_provider=provider,
            ai_model=form.get("ai_model", "").strip(),
        )
        flash(request, "AI override saved.", "success")
    except ServiceError as e:
        flash(request, str(e), "error")
    return _detail(market_id)


@router.post("/admin/supermarkets/{market_id}/delete")
async def market_delete(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    if form.get("confirm") != market_id:
        flash(request, "Type the supermarket ID to confirm deletion.", "error")
        return _detail(market_id)
    st = state(request)
    st.container.scheduler.remove_market_job(market_id)
    await supermarkets.delete_supermarket(st.db, market_id)
    flash(request, f"Deleted supermarket '{market_id}' and its price history.", "success")
    return RedirectResponse("/admin/supermarkets", status_code=303)


@router.get("/admin/cron-preview")
async def cron_preview(request: Request, cron_expression: str = "", tz: str = "") -> Response:
    """htmx fragment: next run times for the expression being typed."""
    timezone = tz or state(request).settings.scheduler_timezone
    error, times = None, []
    if cron_expression.strip():
        try:
            times = describe_times(schedules.preview(cron_expression, timezone), timezone)
        except ServiceError as e:
            error = str(e)
    return render(request, "_cron_preview.html", times=times, error=error)
