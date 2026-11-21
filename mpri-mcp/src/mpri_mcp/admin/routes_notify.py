"""Dashboard: notification channels (Telegram bot, Discord webhook), the crawl summary switch,
price alerts and the log of messages sent."""

# TM - #{ticket number} - MPriMCP Price Alerts & Notifications

from __future__ import annotations

import re

from fastapi import APIRouter, Depends, Request
from fastapi.responses import RedirectResponse
from starlette.responses import Response

from mpri_mcp.admin.web import flash, form_data, render, require_admin, state, verify_csrf
from mpri_mcp.services import alerts
from mpri_mcp.services.errors import ServiceError
from mpri_mcp.services.notifications import (
    DISCORD_WEBHOOK,
    TELEGRAM_TOKEN,
    NotifyError,
    get_notify_settings,
    recent_log,
    update_notify_settings,
    validate_discord_webhook,
    validate_telegram_token,
)

router = APIRouter(dependencies=[Depends(require_admin)])
PAGE = "/admin/notifications"
_CHAT_ID = re.compile(r"^(-?\d{1,20}|@[A-Za-z][A-Za-z0-9_]{4,31})$")


def _back(target: str = PAGE) -> RedirectResponse:
    return RedirectResponse(target, status_code=303)


@router.get(PAGE)
async def notifications_page(request: Request) -> Response:
    st = state(request)
    notifier = st.container.notifier
    secrets = notifier.secrets
    return render(
        request,
        "notifications.html",
        ns=await get_notify_settings(st.db),
        telegram_token=(secrets.source(TELEGRAM_TOKEN), secrets.hint(TELEGRAM_TOKEN)),
        discord_webhook=(secrets.source(DISCORD_WEBHOOK), secrets.hint(DISCORD_WEBHOOK)),
        active=await notifier.active_channels(),
        alerts=await alerts.list_alerts(st.db),
        log=await recent_log(st.db, 30),
    )


@router.post(f"{PAGE}/telegram")
async def save_telegram(request: Request) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    secrets = st.container.notifier.secrets
    chat_id = form.get("chat_id", "").strip()
    try:
        if chat_id and not _CHAT_ID.match(chat_id):
            raise NotifyError("Chat ID is a number (e.g. 123456789 or -100…) or @channelname.")
        if secrets.source(TELEGRAM_TOKEN) != "environment":
            if form.get("remove_token") == "on":
                secrets.set(TELEGRAM_TOKEN, None)
            elif form.get("bot_token", "").strip():
                secrets.set(TELEGRAM_TOKEN, validate_telegram_token(form["bot_token"]))
        await update_notify_settings(
            st.db, telegram_enabled=form.get("enabled") == "on", telegram_chat_id=chat_id or None
        )
        flash(request, "Telegram settings saved.", "success")
    except NotifyError as e:
        flash(request, f"Not saved: {e}", "error")
    return _back()


@router.post(f"{PAGE}/telegram/detect")
async def detect_telegram(request: Request) -> Response:
    await verify_csrf(request)
    st = state(request)
    try:
        chats = await st.container.notifier.detect_telegram_chats()
    except NotifyError as e:
        flash(request, str(e), "error")
        return _back()
    if not chats:
        flash(
            request,
            "No chat found yet. Open your bot in Telegram, send /start, then try again.",
            "warning",
        )
    elif len(chats) == 1:
        chat_id, name = chats[0]
        await update_notify_settings(st.db, telegram_chat_id=chat_id)
        flash(request, f"Chat ID set to {chat_id} ({name}).", "success")
    else:
        listed = ", ".join(f"{cid} ({name})" for cid, name in chats)
        flash(request, f"Several chats found: {listed}. Enter the one to use.", "warning")
    return _back()


@router.post(f"{PAGE}/discord")
async def save_discord(request: Request) -> Response:
    await verify_csrf(request)
    st = state(request)
    form = await form_data(request)
    secrets = st.container.notifier.secrets
    try:
        if secrets.source(DISCORD_WEBHOOK) != "environment":
            if form.get("remove_webhook") == "on":
                secrets.set(DISCORD_WEBHOOK, None)
            elif form.get("webhook_url", "").strip():
                secrets.set(DISCORD_WEBHOOK, validate_discord_webhook(form["webhook_url"]))
        await update_notify_settings(st.db, discord_enabled=form.get("enabled") == "on")
        flash(request, "Discord settings saved.", "success")
    except NotifyError as e:
        flash(request, f"Not saved: {e}", "error")
    return _back()


@router.post(f"{PAGE}/summary")
async def save_summary(request: Request) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    enabled = form.get("enabled") == "on"
    await update_notify_settings(state(request).db, summary_enabled=enabled)
    flash(request, "Crawl summary is on." if enabled else "Crawl summary is off.", "success")
    return _back()


@router.post(PAGE + "/test/{channel}")
async def send_test(request: Request, channel: str) -> Response:
    await verify_csrf(request)
    try:
        await state(request).container.notifier.send_test(channel)
        flash(request, f"Test message sent to {channel.capitalize()}.", "success")
    except NotifyError as e:
        flash(request, f"{channel.capitalize()}: {e}", "error")
    return _back()


@router.post("/admin/products/{product_id}/alert")
async def save_alert(request: Request, product_id: int) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    raw = re.sub(r"[.,\s₫đ]", "", form.get("target_price", ""))
    try:
        if not raw.isdigit():
            raise NotifyError("Enter the price in VND, e.g. 70000.")
        view, created = await alerts.set_alert_for_product(state(request).db, product_id, int(raw))
        flash(
            request,
            f"Alert {'set' if created else 'updated'}: below {int(raw):,} ₫.".replace(",", ".")
            + (" The price is already below it." if view.below else ""),
            "success",
        )
    except (NotifyError, ServiceError) as e:
        flash(request, f"Not saved: {e}", "error")
    return _back(f"/admin/products/{product_id}#alert")


@router.post("/admin/alerts/{alert_id}/delete")
async def delete_alert(request: Request, alert_id: int) -> Response:
    await verify_csrf(request)
    form = await form_data(request)
    await alerts.delete_alert_by_id(state(request).db, alert_id)
    flash(request, "Price alert removed.", "success")
    target = form.get("next", "")
    return _back(target if target.startswith("/admin/") and "//" not in target else PAGE)
