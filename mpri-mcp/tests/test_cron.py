from datetime import datetime
from zoneinfo import ZoneInfo

import pytest

from mpri_mcp.scheduler.cron import CronError, build_trigger, next_fire_times

TZ = "Asia/Ho_Chi_Minh"
# Tuesday 2026-10-20 10:00:30 local.
NOW = datetime(2026, 10, 20, 10, 0, 30, tzinfo=ZoneInfo(TZ))


def _next(expr: str, n: int = 3) -> list[datetime]:
    return [t.astimezone(ZoneInfo(TZ)) for t in next_fire_times(build_trigger(expr, TZ), n, NOW)]


def test_daily_at_3am_local():
    runs = _next("0 3 * * *")
    assert [r.strftime("%Y-%m-%d %H:%M") for r in runs] == [
        "2026-10-21 03:00",
        "2026-10-22 03:00",
        "2026-10-23 03:00",
    ]


@pytest.mark.parametrize("expr", ["0 3 * * 0", "0 3 * * 7", "0 3 * * sun", "0 3 * * SUN"])
def test_sunday_is_standard_cron_sunday(expr):
    assert all(r.strftime("%A") == "Sunday" for r in _next(expr))


def test_weekday_range():
    days = {r.strftime("%a") for r in _next("30 6 * * 1-5", 10)}
    assert days == {"Mon", "Tue", "Wed", "Thu", "Fri"}


def test_range_ending_with_7_includes_sunday():
    days = {r.strftime("%a") for r in _next("0 0 * * 5-7", 6)}
    assert days == {"Fri", "Sat", "Sun"}


def test_step_and_list():
    assert {r.strftime("%a") for r in _next("0 0 * * */2", 8)} == {"Sun", "Tue", "Thu", "Sat"}
    assert {r.strftime("%a") for r in _next("0 0 * * mon,wed", 4)} == {"Mon", "Wed"}


def test_macros():
    assert _next("@daily", 1)[0].strftime("%H:%M") == "00:00"
    assert _next("@weekly", 1)[0].strftime("%A") == "Sunday"


def test_every_15_minutes():
    runs = _next("*/15 * * * *", 4)
    assert [r.strftime("%H:%M") for r in runs] == ["10:15", "10:30", "10:45", "11:00"]


@pytest.mark.parametrize(
    "expr", ["61 * * * *", "* * *", "0 3 * * 8", "0 3 * * 5-1", "0 25 * * *", "a b c d e"]
)
def test_invalid_expressions(expr):
    with pytest.raises(CronError):
        build_trigger(expr, TZ)


def test_invalid_timezone():
    with pytest.raises(CronError):
        build_trigger("0 3 * * *", "Mars/Olympus")
