"""Dashboard: AI provider/model selection, test extraction, usage and price table."""

from __future__ import annotations

import json

from fastapi import APIRouter, Depends, Request
from fastapi.responses import RedirectResponse
from starlette.responses import Response

from mpri_mcp.admin.routes_pages import usage_summary
from mpri_mcp.admin.web import flash, form_data, render, require_admin, state, verify_csrf
from mpri_mcp.services import supermarkets
from mpri_mcp.services.settings import AISettings, get_ai_settings, set_ai_settings, set_prices

router = APIRouter(dependencies=[Depends(require_admin)])


@router.get("/admin/ai")
async def ai_page(request: Request) -> Response:
    st = state(request)
    ai = await get_ai_settings(st.db)
    effective, fallback = await st.container.runner.resolve_ai(None)
    usage = await usage_summary(request, days=7)
    return render(
        request,
        "ai.html",
        ai=ai,
        env={
            "provider": st.settings.ai_provider,
            "model": st.settings.ai_model,
            "fallback_provider": st.settings.ai_fallback_provider,
            "fallback_model": st.settings.ai_fallback_model,
        },
        effective=effective,
        fallback=fallback,
        providers=st.container.registry.statuses(),
        markets=[s.supermarket for s in await supermarkets.list_supermarkets(st.db)],
        usage=usage,
        prices_json=json.dumps(usage["prices"], indent=2),
    )


@router.post("/admin/ai")
async def ai_save(request: Request) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    names = st.container.registry.names()
    for key in ("provider", "fallback_provider"):
        value = form.get(key, "").strip()
        if value and value not in names:
            flash(request, f"Unknown provider '{value}'.", "error")
            return RedirectResponse("/admin/ai", status_code=303)
    await set_ai_settings(
        st.db,
        AISettings(
            provider=form.get("provider", "").strip() or None,
            model=form.get("model", "").strip() or None,
            fallback_provider=form.get("fallback_provider", "").strip() or None,
            fallback_model=form.get("fallback_model", "").strip() or None,
        ),
    )
    flash(request, "AI settings saved. They apply from the next crawl.", "success")
    return RedirectResponse("/admin/ai", status_code=303)


@router.post("/admin/ai/prices")
async def ai_prices(request: Request) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    try:
        parsed = json.loads(form.get("prices", "") or "{}")
        if not isinstance(parsed, dict):
            raise ValueError("expected a JSON object")
        clean = {
            str(model): {"input": float(p["input"]), "output": float(p["output"])}
            for model, p in parsed.items()
        }
    except (ValueError, KeyError, TypeError) as e:
        example = '{"model": {"input": 0.1, "output": 0.4}}'
        flash(request, f"Not saved — use {example}: {e}", "error")
        return RedirectResponse("/admin/ai", status_code=303)
    await set_prices(state(request).db, clean)
    flash(request, "Price table saved.", "success")
    return RedirectResponse("/admin/ai", status_code=303)


@router.post("/admin/ai/test")
async def ai_test(request: Request) -> Response:
    """htmx fragment: run one extraction with the chosen provider/model, without saving."""
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    url = form.get("url", "").strip()
    if not url.startswith(("http://", "https://")):
        return render(request, "_dry_run.html", result=None, error="Enter a full http(s) URL.")
    result = await st.container.runner.dry_run(
        url,
        supermarket_id=form.get("supermarket_id") or None,
        provider=form.get("provider") or None,
        model=form.get("model", "").strip() or None,
    )
    return render(request, "_dry_run.html", result=result)
