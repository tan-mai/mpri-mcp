"""The OAuth consent screen: the admin approves (or denies) a client such as Claude."""

from __future__ import annotations

from urllib.parse import urlparse

from fastapi import APIRouter, Request
from fastapi.responses import RedirectResponse
from mcp.server.auth.provider import construct_redirect_uri
from starlette.responses import Response

from mpri_mcp.admin.web import (
    admin_exists,
    current_admin,
    login_redirect_url,
    render,
    state,
    verify_csrf,
)

router = APIRouter()


def _expired(request: Request) -> Response:
    return render(
        request,
        "error.html",
        status_code=400,
        title="Authorization request expired",
        message="This connection request is no longer valid. Start connecting again from Claude.",
    )


@router.get("/oauth/consent")
async def consent_form(request: Request, req: str = "") -> Response:
    if not await admin_exists(request):
        return RedirectResponse("/admin/setup", status_code=303)
    admin = await current_admin(request)
    if admin is None:
        return RedirectResponse(login_redirect_url(f"/oauth/consent?req={req}"), status_code=303)
    request.state.admin = admin
    pending = await state(request).oauth.get_pending(req)
    if pending is None:
        return _expired(request)
    return render(
        request,
        "consent.html",
        pending=pending,
        client_name=pending.client.client_name or "An unnamed application",
        redirect_host=urlparse(pending.redirect_uri).netloc,
        resource=pending.resource,
    )


@router.post("/oauth/consent")
async def consent_submit(request: Request) -> Response:
    await verify_csrf(request)
    admin = await current_admin(request)
    form = await request.form()
    req = str(form.get("req") or "")
    if admin is None:
        return RedirectResponse(login_redirect_url(f"/oauth/consent?req={req}"), status_code=303)
    st = state(request)
    pending = await st.oauth.get_pending(req)
    if pending is None:
        return _expired(request)
    approve = form.get("decision") == "allow"
    code = await st.oauth.complete_pending(pending, approve=approve, subject=admin.username)
    params: dict[str, str | None] = {"state": pending.state, "iss": st.settings.issuer_url}
    if code is not None:
        params["code"] = code
    else:
        params.update(error="access_denied", error_description="The admin denied access.")
    return RedirectResponse(
        construct_redirect_uri(pending.redirect_uri, **params),
        status_code=303,
        headers={"Cache-Control": "no-store"},
    )
