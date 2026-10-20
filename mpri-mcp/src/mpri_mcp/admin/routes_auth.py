"""First-run setup, login/logout and account (password) pages."""

from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Request
from fastapi.responses import RedirectResponse
from starlette.responses import Response

from mpri_mcp.admin.web import (
    admin_exists,
    client_ip,
    current_admin,
    flash,
    login_session,
    logout_session,
    render,
    require_admin,
    safe_next,
    state,
    verify_csrf,
)
from mpri_mcp.auth.admin import (
    AdminExistsError,
    authenticate,
    bump_session_version,
    create_admin,
    set_password,
)
from mpri_mcp.auth.passwords import (
    MIN_PASSWORD_LENGTH,
    validate_new_password,
    validate_username,
    verify_password,
)
from mpri_mcp.db.models import AdminUser

router = APIRouter()


def _form_str(form: object, key: str) -> str:
    value = form.get(key)  # type: ignore[attr-defined]
    return value if isinstance(value, str) else ""


# --------------------------------------------------------------------------- setup
@router.get("/admin/setup")
async def setup_form(request: Request) -> Response:
    if await admin_exists(request):
        raise HTTPException(status_code=404)
    return render(request, "setup.html", min_length=MIN_PASSWORD_LENGTH, username="")


@router.post("/admin/setup")
async def setup_submit(request: Request) -> Response:
    if await admin_exists(request):
        raise HTTPException(status_code=404)
    await verify_csrf(request)
    st = state(request)
    form = await request.form()
    token, username = _form_str(form, "setup_token"), _form_str(form, "username").strip()
    password, confirm = _form_str(form, "password"), _form_str(form, "confirm")
    ip_key = f"setup:{client_ip(request)}"

    def fail(message: str) -> Response:
        return render(
            request,
            "setup.html",
            status_code=400,
            error=message,
            min_length=MIN_PASSWORD_LENGTH,
            username=username,
        )

    if st.login_limiter.blocked(ip_key):
        return fail("Too many attempts. Wait 15 minutes and try again.")
    if not st.setup_tokens.verify(token):
        st.login_limiter.record_failure(ip_key)
        return fail("The setup token is wrong. Copy it from the server logs.")
    error = validate_username(username) or validate_new_password(password, confirm, username)
    if error:
        return fail(error)
    try:
        user = await create_admin(st.db, username, password)
    except AdminExistsError:
        raise HTTPException(status_code=404) from None
    st.admin_exists = True
    st.setup_tokens.clear()
    login_session(request, user)
    flash(request, f"Welcome, {user.username}. Your admin account is ready.", "success")
    return RedirectResponse("/admin", status_code=303)


# --------------------------------------------------------------------------- login
@router.get("/admin/login")
async def login_form(request: Request, next: str | None = None) -> Response:
    if not await admin_exists(request):
        return RedirectResponse("/admin/setup", status_code=303)
    if await current_admin(request):
        return RedirectResponse(safe_next(next), status_code=303)
    return render(request, "login.html", next=safe_next(next), username="")


@router.post("/admin/login")
async def login_submit(request: Request) -> Response:
    if not await admin_exists(request):
        return RedirectResponse("/admin/setup", status_code=303)
    await verify_csrf(request)
    st = state(request)
    form = await request.form()
    username, password = _form_str(form, "username").strip(), _form_str(form, "password")
    next_url = safe_next(_form_str(form, "next"))
    keys = (f"ip:{client_ip(request)}", f"user:{username.lower()}")

    if st.login_limiter.blocked(*keys):
        return render(
            request,
            "login.html",
            status_code=429,
            next=next_url,
            username=username,
            error="Too many failed attempts. Wait 15 minutes and try again.",
        )
    user = await authenticate(st.db, username, password)
    if user is None:
        st.login_limiter.record_failure(*keys)
        return render(
            request,
            "login.html",
            status_code=401,
            next=next_url,
            username=username,
            error="Invalid username or password.",
        )
    st.login_limiter.reset(*keys)
    login_session(request, user)
    return RedirectResponse(next_url, status_code=303)


@router.post("/admin/logout")
async def logout(request: Request) -> Response:
    await verify_csrf(request)
    logout_session(request)
    return RedirectResponse("/admin/login", status_code=303)


# --------------------------------------------------------------------------- account
@router.get("/admin/account")
async def account(request: Request, admin: AdminUser = Depends(require_admin)) -> Response:
    return render(request, "account.html", min_length=MIN_PASSWORD_LENGTH)


@router.post("/admin/account/password")
async def change_password(request: Request, admin: AdminUser = Depends(require_admin)) -> Response:
    await verify_csrf(request)
    form = await request.form()
    current, new = _form_str(form, "current"), _form_str(form, "password")
    confirm = _form_str(form, "confirm")
    error = None
    if not verify_password(admin.password_hash, current):
        error = "Current password is wrong."
    else:
        error = validate_new_password(new, confirm, admin.username)
    if error:
        return render(
            request, "account.html", status_code=400, error=error, min_length=MIN_PASSWORD_LENGTH
        )
    user = await set_password(state(request).db, username=admin.username, password=new)
    login_session(request, user)  # keep this browser signed in; all others are logged out
    flash(request, "Password changed. Other sessions have been signed out.", "success")
    return RedirectResponse("/admin/account", status_code=303)


@router.post("/admin/account/logout-all")
async def logout_everywhere(
    request: Request, admin: AdminUser = Depends(require_admin)
) -> Response:
    await verify_csrf(request)
    await bump_session_version(state(request).db, admin.id)
    logout_session(request)
    return RedirectResponse("/admin/login", status_code=303)
