"""Standard 5-field cron expressions -> APScheduler 3 CronTrigger.

APScheduler 3's `from_crontab` treats day-of-week 0 as Monday, while standard cron (and
anything Claude or a person writes) means Sunday. We expand the day-of-week field ourselves
and hand APScheduler unambiguous weekday names.
"""

from __future__ import annotations

from datetime import datetime
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from apscheduler.triggers.cron import CronTrigger

from mpri_mcp.i18n import format_weekday_datetime
from mpri_mcp.utils.time import utcnow

MACROS = {
    "@yearly": "0 0 1 1 *",
    "@annually": "0 0 1 1 *",
    "@monthly": "0 0 1 * *",
    "@weekly": "0 0 * * 0",
    "@daily": "0 0 * * *",
    "@midnight": "0 0 * * *",
    "@hourly": "0 * * * *",
}
# Standard cron numbering: 0 = Sunday ... 6 = Saturday (7 is also Sunday).
_DOW_NAMES = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
_DOW_NUM = {name: i for i, name in enumerate(_DOW_NAMES)}


class CronError(ValueError):
    pass


def _dow_value(token: str) -> int:
    token = token.lower()
    if token in _DOW_NUM:
        return _DOW_NUM[token]
    if token.isdigit() and 0 <= int(token) <= 7:
        return int(token) % 7
    raise CronError(f"invalid day-of-week value '{token}'")


def _expand_dow(field: str) -> set[int]:
    days: set[int] = set()
    for part in field.split(","):
        if not part:
            raise CronError("empty item in day-of-week field")
        base, _, step_s = part.partition("/")
        step = 1
        if step_s:
            if not step_s.isdigit() or int(step_s) == 0:
                raise CronError(f"invalid step '{step_s}' in day-of-week field")
            step = int(step_s)
        if base in ("*", "?"):
            start, end = 0, 6
        elif "-" in base:
            a, b = base.split("-", 1)
            start, end = _dow_value(a), _dow_value(b)
            if b == "7":  # "1-7" means Monday..Sunday
                end = 7
            if end < start:
                raise CronError(f"descending range '{base}' in day-of-week field")
        else:
            start = end = _dow_value(base)
            if step_s:  # "1/2" means from Monday every 2 days
                end = 6
        days.update(d % 7 for d in range(start, end + 1, step))
    return days


def _translate_dow(field: str) -> str:
    days = _expand_dow(field)
    if days == set(range(7)):
        return "*"
    # APScheduler accepts comma-separated weekday names in any order.
    return ",".join(_DOW_NAMES[d] for d in sorted(days))


def normalize_expression(expr: str) -> str:
    expr = " ".join(expr.split())
    return MACROS.get(expr.lower(), expr)


def build_trigger(expr: str, timezone: str) -> CronTrigger:
    """Parse a standard cron expression; raises CronError with a readable message."""
    expr = normalize_expression(expr)
    fields = expr.split(" ")
    if len(fields) != 5:
        raise CronError(
            f"expected 5 fields (minute hour day month day-of-week), got {len(fields)}: '{expr}'"
        )
    try:
        tz = ZoneInfo(timezone)
    except (ZoneInfoNotFoundError, ValueError) as e:
        raise CronError(f"unknown timezone '{timezone}'") from e
    minute, hour, day, month, dow = fields
    try:
        return CronTrigger(
            minute=minute,
            hour=hour,
            day=day,
            month=month,
            day_of_week=_translate_dow(dow),
            timezone=tz,
        )
    except CronError:
        raise
    except ValueError as e:
        raise CronError(str(e)) from e


def next_fire_times(
    trigger: CronTrigger, count: int = 3, now: datetime | None = None
) -> list[datetime]:
    times: list[datetime] = []
    previous = None
    current = now or utcnow()
    for _ in range(count):
        nxt = trigger.get_next_fire_time(previous, current)
        if nxt is None:
            break
        times.append(nxt)
        previous = nxt
        current = nxt
    return times


def describe_times(times: list[datetime], timezone: str) -> list[str]:
    """In the current language: 'Sat 24/10/2026 03:00' / 'Thứ Bảy, 24/10/2026 03:00'."""
    return [format_weekday_datetime(t, timezone) or "" for t in times]
