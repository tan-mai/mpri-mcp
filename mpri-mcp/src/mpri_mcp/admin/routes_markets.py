"""Dashboard: supermarkets, their schedules and search URL templates."""

from __future__ import annotations

from fastapi import APIRouter, Depends, Request
from fastapi.responses import RedirectResponse
from starlette.responses import Response

from mpri_mcp.admin.web import flash, form_data, render, require_admin, state, verify_csrf
from mpri_mcp.scheduler.cron import describe_times
from mpri_mcp.services import agent_tasks, runs, schedules, supermarkets
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
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        market = await supermarkets.add_supermarket(
            st.db,
            form.get("name", ""),
            form.get("base_url", ""),
            form.get("search_url_template", ""),
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


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
def _queued_message(r: agent_tasks.QueueResult) -> tuple[str, str]:
    if r.queued:
        return (
            f"Queued {r.queued} crawl task(s) for {r.supermarket.name}. Claude reads them "
            "with the browser (ask it to run the agent crawl).",
            "success",
        )
    return (
        f"{r.supermarket.name}: every tracked page already has an open crawl task.",
        "warning",
    )


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
@router.post("/admin/supermarkets/crawl")
async def market_crawl_selected(request: Request) -> Response:
    """Queue crawl tasks for every ticked supermarket on the list page."""
    await verify_csrf(request)
    st = state(request)
    ids = list(dict.fromkeys(str(v) for v in (await request.form()).getlist("market_id")))
    if not ids:
        flash(request, "Select at least one supermarket to crawl.", "warning")
        return RedirectResponse("/admin/supermarkets", status_code=303)
    for market_id in ids:
        try:
            result = await schedules.trigger_now(st.db, st.settings, market_id)
            flash(request, *_queued_message(result))
        except ServiceError as e:
            flash(request, f"{market_id}: {e}", "error")
    return RedirectResponse("/admin/supermarkets", status_code=303)


@router.get("/admin/supermarkets/{market_id}")
async def market_detail(request: Request, market_id: str) -> Response:
    st = state(request)
    market = await supermarkets.get_supermarket(st.db, market_id)
    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    await agent_tasks.maintain(st.db, st.settings)
    summary = next(
        s for s in await supermarkets.list_supermarkets(st.db) if s.supermarket.id == market_id
    )
    return render(
        request,
        "supermarket_detail.html",
        market=market,
        summary=summary,
        urls=sorted(await supermarkets.market_urls(st.db, market_id), key=lambda u: u.product.name),
        recent=await runs.recent_runs(st.db, 10, market_id),
        next_run=st.container.scheduler.next_run_time(market_id),
        running=st.container.scheduler.is_running(market_id),
    )


@router.post("/admin/supermarkets/{market_id}")
async def market_edit(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    try:
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        changes = {"name": form.get("name", ""), "base_url": form.get("base_url", "")}
        if "search_url_template" in form:
            changes["search_url_template"] = form["search_url_template"]
        await supermarkets.update_supermarket(state(request).db, market_id, **changes)
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


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
@router.post("/admin/supermarkets/{market_id}/run")
async def market_run(request: Request, market_id: str) -> Response:
    await verify_csrf(request)
    st = state(request)
    try:
        flash(request, *_queued_message(await schedules.trigger_now(st.db, st.settings, market_id)))
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
