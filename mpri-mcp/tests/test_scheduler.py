import asyncio

import pytest

from mpri_mcp.scheduler.backup import backup_sqlite
from mpri_mcp.scheduler.engine import SchedulerEngine, market_job_id
from mpri_mcp.services import products, schedules, supermarkets
from mpri_mcp.services.errors import NotFoundError, ValidationError
from mpri_mcp.services.products import UrlInput


class FakeCrawler:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str]] = []
        self.release = asyncio.Event()
        self.started = asyncio.Event()

    async def __call__(self, supermarket_id: str, trigger: str) -> None:
        self.calls.append((supermarket_id, trigger))
        self.started.set()
        await self.release.wait()


@pytest.fixture
async def engine(settings, db):
    crawler = FakeCrawler()
    eng = SchedulerEngine(settings, crawler)
    eng.crawler = crawler  # type: ignore[attr-defined]
    await eng.start(db)
    yield eng
    crawler.release.set()
    await eng.shutdown()


async def test_set_cron_registers_job_without_restart(db, engine):
    market = await supermarkets.add_supermarket(db, "Lotte Mart", "lottemart.vn")
    assert engine.next_run_time(market.id) is None

    result = await schedules.set_cron(db, engine, market.id, "0 3 * * *")
    assert result.cron_expression == "0 3 * * *"
    assert len(result.next_runs) == 3
    first = engine.next_run_time(market.id)
    assert first is not None and first.strftime("%H:%M") == "03:00"

    await schedules.set_cron(db, engine, market.id, "30 6 * * *")
    assert engine.next_run_time(market.id).strftime("%H:%M") == "06:30"

    stored = await supermarkets.get_supermarket(db, market.id)
    assert stored.cron_expression == "30 6 * * *" and stored.cron_enabled


async def test_invalid_cron_leaves_db_unchanged(db, engine):
    market = await supermarkets.add_supermarket(db, "Winmart", "winmart.vn")
    await schedules.set_cron(db, engine, market.id, "0 2 * * *")
    with pytest.raises(ValidationError):
        await schedules.set_cron(db, engine, market.id, "0 99 * * *")
    stored = await supermarkets.get_supermarket(db, market.id)
    assert stored.cron_expression == "0 2 * * *"
    assert engine.next_run_time(market.id).strftime("%H:%M") == "02:00"


async def test_unknown_market(db, engine):
    with pytest.raises(NotFoundError):
        await schedules.set_cron(db, engine, "nope", "0 3 * * *")


async def test_disable_keeps_expression(db, engine):
    market = await supermarkets.add_supermarket(db, "Go!", "go-vietnam.vn")
    await schedules.set_cron(db, engine, market.id, "0 0 * * *")
    await schedules.disable_cron(db, engine, market.id)
    assert engine.next_run_time(market.id) is None
    stored = await supermarkets.get_supermarket(db, market.id)
    assert stored.cron_expression == "0 0 * * *" and not stored.cron_enabled


async def test_jobs_restored_after_restart(settings, db, engine):
    a = await supermarkets.add_supermarket(db, "A Mart", "a.vn")
    b = await supermarkets.add_supermarket(db, "B Mart", "b.vn")
    await schedules.set_cron(db, engine, a.id, "0 1 * * *")
    await schedules.set_cron(db, engine, b.id, "0 2 * * *")
    await schedules.disable_cron(db, engine, b.id)

    fresh = SchedulerEngine(settings, FakeCrawler())
    await fresh.start(db)
    try:
        assert fresh.next_run_time(a.id).strftime("%H:%M") == "01:00"
        assert fresh.next_run_time(b.id) is None
    finally:
        await fresh.shutdown()


async def test_trigger_now_runs_in_background_and_refuses_overlap(db, engine):
    market = await supermarkets.add_supermarket(db, "MM", "online.mmvietnam.com")
    with pytest.raises(ValidationError):  # no URLs yet
        await schedules.trigger_now(db, engine, market.id)

    await products.add_product_with_urls(
        db, "Satori 500ml", None, [UrlInput(market.id, "https://online.mmvietnam.com/p.html")]
    )
    assert await schedules.trigger_now(db, engine, market.id) is True
    await asyncio.wait_for(engine.crawler.started.wait(), 2)
    assert engine.is_running(market.id)
    assert await schedules.trigger_now(db, engine, market.id) is False

    engine.crawler.release.set()
    for _ in range(50):
        if not engine.is_running(market.id):
            break
        await asyncio.sleep(0.01)
    assert not engine.is_running(market.id)
    assert engine.crawler.calls == [(market.id, "manual")]


async def test_cron_job_function_skips_when_already_running(db, engine):
    market = await supermarkets.add_supermarket(db, "MM", "online.mmvietnam.com")
    await products.add_product_with_urls(
        db, "Satori 500ml", None, [UrlInput(market.id, "https://online.mmvietnam.com/p.html")]
    )
    assert engine.run_now(market.id)
    await asyncio.wait_for(engine.crawler.started.wait(), 2)
    # Simulate the cron job firing while the manual run is in progress.
    await engine._run_market(market.id, "cron")
    assert engine.crawler.calls == [(market.id, "manual")]


async def test_job_id_format():
    assert market_job_id("lotte-mart") == "crawl:lotte-mart"


async def test_backup_writes_and_prunes(settings, db, tmp_path):
    await supermarkets.add_supermarket(db, "Lotte Mart", "lottemart.vn")
    backup_dir = tmp_path / "backups"
    for i in range(4):
        (backup_dir).mkdir(exist_ok=True)
        (backup_dir / f"mpri-2020010{i}-000000.db").write_bytes(b"old")
    dest = await backup_sqlite(db.sqlite_path, backup_dir, keep=2)
    remaining = sorted(p.name for p in backup_dir.glob("mpri-*.db"))
    assert len(remaining) == 2 and dest.name in remaining

    import sqlite3

    con = sqlite3.connect(dest)
    try:
        assert con.execute("select name from supermarkets").fetchall() == [("Lotte Mart",)]
    finally:
        con.close()
