"""Notifications to the user's own Telegram bot and/or Discord webhook: price alerts and the
summary sent after each finished agent crawl.

Secrets (bot token, webhook URL) are kept out of the database: they are entered on the
dashboard and stored as files in DATA_DIR/secrets, or come from the environment. Sending
happens in the background, never slows down or breaks the tool call, and every attempt is
recorded in notification_log."""

# TM - #{ticket number} - MPriMCP Price Alerts & Notifications

from __future__ import annotations

import asyncio
import html
import logging
import re
from collections.abc import Callable, Coroutine
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

import httpx2
from sqlalchemy import select

from mpri_mcp.config import Settings
from mpri_mcp.db import Database
from mpri_mcp.db.models import AppSetting, NotificationLog
from mpri_mcp.services import alerts
from mpri_mcp.services.notify_messages import Line, alert_lines, plain, summary_lines, test_lines
from mpri_mcp.utils.time import utcnow

log = logging.getLogger(__name__)

CHANNELS = ("telegram", "discord")
SETTINGS_KEY = "notifications"
TELEGRAM_API = "https://api.telegram.org"
TELEGRAM_LIMIT = 4096
DISCORD_LIMIT = 2000
DISCORD_PREFIXES = (
    "https://discord.com/api/webhooks/",
    "https://discordapp.com/api/webhooks/",
    "https://ptb.discord.com/api/webhooks/",
    "https://canary.discord.com/api/webhooks/",
)
TELEGRAM_TOKEN = "telegram_bot_token"
DISCORD_WEBHOOK = "discord_webhook_url"
_TOKEN_RE = re.compile(r"^\d{5,}:[A-Za-z0-9_-]{30,}$")
_DISCORD_ESCAPE = re.compile(r"([\\*_~`|>\[\]])")


class NotifyError(Exception):
    pass


class _RetryAfter(Exception):
    def __init__(self, seconds: float, message: str) -> None:
        super().__init__(message)
        self.seconds = seconds


class _Transient(Exception):
    pass


# ---------------------------------------------------------------------- settings


@dataclass
class NotifySettings:
    telegram_enabled: bool = False
    telegram_chat_id: str | None = None
    discord_enabled: bool = False
    summary_enabled: bool = True
    last_summary_at: str | None = None  # ISO time of the last finished crawl round

    @property
    def since(self) -> datetime | None:
        return datetime.fromisoformat(self.last_summary_at) if self.last_summary_at else None


async def get_notify_settings(db: Database) -> NotifySettings:
    async with db.session() as s:
        row = await s.get(AppSetting, SETTINGS_KEY)
    raw = row.value if row is not None and isinstance(row.value, dict) else {}
    known = NotifySettings.__dataclass_fields__
    return NotifySettings(**{k: v for k, v in raw.items() if k in known})


async def update_notify_settings(db: Database, **changes: Any) -> NotifySettings:
    current = await get_notify_settings(db)
    for key, value in changes.items():
        if key not in NotifySettings.__dataclass_fields__:
            raise ValueError(f"Unknown notification setting '{key}'")
        setattr(current, key, value)
    async with db.session() as s:
        row = await s.get(AppSetting, SETTINGS_KEY)
        if row is None:
            s.add(AppSetting(key=SETTINGS_KEY, value=asdict(current)))
        else:
            row.value = asdict(current)
    return current


def validate_telegram_token(token: str) -> str:
    token = token.strip()
    if not _TOKEN_RE.match(token):
        raise NotifyError(
            "That does not look like a Telegram bot token (from @BotFather, e.g. '123456789:AA…')."
        )
    return token


def validate_discord_webhook(url: str) -> str:
    url = url.strip()
    if not url.startswith(DISCORD_PREFIXES):
        raise NotifyError(
            "Paste the Discord webhook URL (Channel settings → Integrations → Webhooks → "
            "Copy Webhook URL); it starts with https://discord.com/api/webhooks/."
        )
    return url


class SecretStore:
    """Bot token and webhook URL: environment first, then files under DATA_DIR/secrets."""

    def __init__(self, settings: Settings) -> None:
        self._dir: Path = settings.data_dir / "secrets"
        self._env = {
            TELEGRAM_TOKEN: settings.telegram_bot_token,
            DISCORD_WEBHOOK: settings.discord_webhook_url,
        }

    def _path(self, name: str) -> Path:
        return self._dir / name

    def get(self, name: str) -> str | None:
        env = self._env.get(name)
        if env is not None:
            return env.get_secret_value()
        path = self._path(name)
        return path.read_text(encoding="utf-8").strip() or None if path.exists() else None

    def source(self, name: str) -> str | None:
        if self._env.get(name) is not None:
            return "environment"
        return "dashboard" if self._path(name).exists() else None

    def hint(self, name: str) -> str | None:
        value = self.get(name)
        return f"…{value[-4:]}" if value else None

    def set(self, name: str, value: str | None) -> None:
        path = self._path(name)
        if not value:
            path.unlink(missing_ok=True)
            return
        self._dir.mkdir(parents=True, exist_ok=True)
        path.write_text(value, encoding="utf-8")
        try:
            path.chmod(0o600)
        except OSError:  # Windows / restricted filesystems
            pass


# ---------------------------------------------------------------------- rendering


def render_telegram(line: Line) -> str:
    out = []
    for seg in line:
        text = html.escape(seg.text, quote=False)
        if seg.url:
            text = f'<a href="{html.escape(seg.url)}">{text}</a>'
        if seg.bold:
            text = f"<b>{text}</b>"
        out.append(text)
    return "".join(out)


def render_discord(line: Line) -> str:
    out = []
    for seg in line:
        text = _DISCORD_ESCAPE.sub(r"\\\1", seg.text)
        if seg.url:
            text = f"[{text}](<{seg.url}>)"  # <...> keeps Discord from adding a link preview
        if seg.bold:
            text = f"**{text}**"
        out.append(text)
    return "".join(out)


def split_messages(lines: list[Line], render: Callable[[Line], str], limit: int) -> list[str]:
    """Rendered lines packed into as few messages as the channel's length limit allows."""
    messages: list[str] = []
    current = ""
    for line in lines:
        text = render(line)
        if len(text) > limit:
            text = text[: limit - 1] + "…"
        candidate = f"{current}\n{text}" if current else text
        if len(candidate) > limit:
            messages.append(current)
            current = text
        else:
            current = candidate
    if current:
        messages.append(current)
    return messages


# ---------------------------------------------------------------------- sending


@dataclass
class AfterSubmit:
    alerts: list[alerts.AlertHit] = field(default_factory=list)
    channels: list[str] = field(default_factory=list)
    summary_sent_to: list[str] = field(default_factory=list)


class Notifier:
    def __init__(self, settings: Settings, db: Database) -> None:
        self._settings = settings
        self._db = db
        self.secrets = SecretStore(settings)
        self.backoff: tuple[float, ...] = (2.0, 5.0, 15.0)
        self.client_factory: Callable[[], httpx2.AsyncClient] = lambda: httpx2.AsyncClient(
            timeout=settings.notify_timeout_seconds
        )
        self._tasks: set[asyncio.Task[None]] = set()

    # -------------------------------------------------------------- configuration
    async def active_channels(self) -> list[str]:
        ns = await get_notify_settings(self._db)
        channels = []
        if ns.telegram_enabled and ns.telegram_chat_id and self.secrets.get(TELEGRAM_TOKEN):
            channels.append("telegram")
        if ns.discord_enabled and self.secrets.get(DISCORD_WEBHOOK):
            channels.append("discord")
        return channels

    async def detect_telegram_chats(self) -> list[tuple[str, str]]:
        """Chats that recently messaged the bot: (chat_id, name). Send /start to it first."""
        token = self.secrets.get(TELEGRAM_TOKEN)
        if not token:
            raise NotifyError("Save the Telegram bot token first.")
        try:
            async with self.client_factory() as client:
                resp = await client.get(f"{TELEGRAM_API}/bot{token}/getUpdates")
        except httpx2.HTTPError as e:
            raise NotifyError(self._clean(f"Telegram is unreachable: {e}")) from e
        data = resp.json() if resp.content else {}
        if resp.status_code >= 400 or not data.get("ok"):
            raise NotifyError(f"Telegram: {data.get('description') or resp.status_code}")
        chats: dict[str, str] = {}
        for update in data.get("result", []):
            msg = update.get("message") or update.get("channel_post") or {}
            chat = msg.get("chat") or {}
            if "id" in chat:
                name = chat.get("title") or " ".join(
                    x for x in (chat.get("first_name"), chat.get("last_name")) if x
                )
                chats[str(chat["id"])] = name or chat.get("username") or str(chat["id"])
        return list(chats.items())

    # -------------------------------------------------------------- public sending
    async def notify(self, kind: str, lines: list[Line]) -> list[str]:
        """Queue a message to every active channel; returns those channels."""
        channels = await self.active_channels()
        for channel in channels:
            self._spawn(self._deliver(kind, channel, lines))
        return channels

    async def send_test(self, channel: str) -> None:
        if channel not in CHANNELS:
            raise NotifyError(f"Unknown channel '{channel}'.")
        if channel == "telegram":
            ns = await get_notify_settings(self._db)
            if not self.secrets.get(TELEGRAM_TOKEN) or not ns.telegram_chat_id:
                raise NotifyError("Save the Telegram bot token and chat ID first.")
        elif not self.secrets.get(DISCORD_WEBHOOK):
            raise NotifyError("Save the Discord webhook URL first.")
        await self._deliver("test", channel, test_lines(), raise_errors=True)

    async def after_submit(
        self, saved: list[alerts.SavedPrice], queue_empty: bool, accepted: int
    ) -> AfterSubmit:
        """After submit_price_results: price alerts for the new prices, and the summary once
        the crawl queue is empty."""
        out = AfterSubmit()
        try:
            out.channels = await self.active_channels()
            out.alerts = await alerts.evaluate(self._db, saved)
            for hit in out.alerts:
                await self.notify("alert", alert_lines(hit))
            if queue_empty and accepted:
                ns = await get_notify_settings(self._db)
                now = utcnow()
                if ns.summary_enabled:
                    lines = await summary_lines(
                        self._db, ns.since, now, self._settings.scheduler_timezone
                    )
                    out.summary_sent_to = await self.notify("summary", lines)
                await update_notify_settings(self._db, last_summary_at=now.isoformat())
        except Exception:  # notifications must never break the agent crawl
            log.exception("Notifications after submit failed")
        return out

    async def drain(self) -> None:
        while self._tasks:
            await asyncio.gather(*list(self._tasks), return_exceptions=True)

    async def stop(self, grace_seconds: float = 5) -> None:
        tasks = list(self._tasks)
        if tasks:
            _, pending = await asyncio.wait(tasks, timeout=grace_seconds)
            for task in pending:
                task.cancel()

    # -------------------------------------------------------------- internals
    def _spawn(self, coro: Coroutine[Any, Any, None]) -> None:
        task = asyncio.create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    def _clean(self, text: str) -> str:
        for name in (TELEGRAM_TOKEN, DISCORD_WEBHOOK):
            secret = self.secrets.get(name)
            if secret:
                text = text.replace(secret, "***")
        return text[:500]

    async def _deliver(
        self, kind: str, channel: str, lines: list[Line], raise_errors: bool = False
    ) -> None:
        error = None
        try:
            if channel == "telegram":
                texts = split_messages(lines, render_telegram, TELEGRAM_LIMIT)
            else:
                texts = split_messages(lines, render_discord, DISCORD_LIMIT)
            async with self.client_factory() as client:
                for text in texts:
                    await self._send_with_retry(client, channel, text)
        except Exception as e:
            error = self._clean(str(e) or type(e).__name__)
            log.warning("Notification (%s) to %s failed: %s", kind, channel, error)
        else:
            log.info("Notification (%s) sent to %s", kind, channel)
        async with self._db.session() as s:
            s.add(
                NotificationLog(
                    kind=kind,
                    channel=channel,
                    status="failed" if error else "sent",
                    message=plain(lines)[:1000],
                    error=error,
                )
            )
        if raise_errors and error:
            raise NotifyError(error)

    async def _send_with_retry(self, client: httpx2.AsyncClient, channel: str, text: str) -> None:
        for attempt in range(len(self.backoff) + 1):
            try:
                await self._post(client, channel, text)
                return
            except _RetryAfter as e:
                wait = min(e.seconds, 60.0)
                last = e
            except (_Transient, httpx2.TransportError) as e:
                wait = self.backoff[attempt] if attempt < len(self.backoff) else 0
                last = e
            if attempt == len(self.backoff):
                raise NotifyError(str(last))
            await asyncio.sleep(wait)

    async def _post(self, client: httpx2.AsyncClient, channel: str, text: str) -> None:
        if channel == "telegram":
            ns = await get_notify_settings(self._db)
            token = self.secrets.get(TELEGRAM_TOKEN)
            resp = await client.post(
                f"{TELEGRAM_API}/bot{token}/sendMessage",
                json={
                    "chat_id": ns.telegram_chat_id,
                    "text": text,
                    "parse_mode": "HTML",
                    "disable_web_page_preview": True,
                },
            )
            data = _json(resp)
            if resp.status_code == 429:
                retry = float((data.get("parameters") or {}).get("retry_after", 5))
                raise _RetryAfter(retry, "Telegram rate limit")
            if resp.status_code >= 500:
                raise _Transient(f"Telegram HTTP {resp.status_code}")
            if resp.status_code >= 400 or not data.get("ok", False):
                raise NotifyError(f"Telegram: {data.get('description') or resp.status_code}")
        else:
            url = self.secrets.get(DISCORD_WEBHOOK)
            resp = await client.post(
                url or "", json={"content": text, "allowed_mentions": {"parse": []}}
            )
            if resp.status_code == 429:
                retry = float(_json(resp).get("retry_after", 5))
                raise _RetryAfter(retry, "Discord rate limit")
            if resp.status_code >= 500:
                raise _Transient(f"Discord HTTP {resp.status_code}")
            if resp.status_code >= 400:
                message = _json(resp).get("message") or resp.text[:200]
                raise NotifyError(f"Discord HTTP {resp.status_code}: {message}")


def _json(resp: httpx2.Response) -> dict[str, Any]:
    try:
        data = resp.json()
    except ValueError:
        return {}
    return data if isinstance(data, dict) else {}


async def recent_log(db: Database, limit: int = 30) -> list[NotificationLog]:
    async with db.session() as s:
        return list(
            (
                await s.scalars(
                    select(NotificationLog).order_by(NotificationLog.id.desc()).limit(limit)
                )
            ).all()
        )
