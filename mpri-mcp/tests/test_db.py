"""Existing databases get columns added after the first release, without losing data."""

from sqlalchemy import inspect, text

from mpri_mcp.db import Database
from mpri_mcp.services import supermarkets

# The supermarkets table as released before wait_for_api / api_url_contains existed.
OLD_SUPERMARKETS = """
CREATE TABLE supermarkets (
    id VARCHAR(64) NOT NULL PRIMARY KEY,
    name VARCHAR(200) NOT NULL UNIQUE,
    base_url VARCHAR(500) NOT NULL,
    cron_expression VARCHAR(100),
    cron_enabled BOOLEAN NOT NULL,
    timezone VARCHAR(64) NOT NULL,
    crawl_config JSON NOT NULL,
    ai_provider VARCHAR(50),
    ai_model VARCHAR(100),
    created_at DATETIME NOT NULL,
    updated_at DATETIME NOT NULL
)
"""


async def _columns(db: Database, table: str) -> set[str]:
    async with db.engine.connect() as conn:
        cols = await conn.run_sync(lambda c: inspect(c).get_columns(table))
    return {c["name"] for c in cols}


async def test_create_all_adds_new_columns_to_an_existing_database(settings):
    db = Database(settings.resolved_database_url)
    try:
        async with db.engine.begin() as conn:
            await conn.execute(text(OLD_SUPERMARKETS))
            await conn.execute(
                text(
                    "INSERT INTO supermarkets VALUES ('bach-hoa-xanh', 'Bách Hóa Xanh', "
                    "'https://www.bachhoaxanh.com', '0 10 * * 2,6', 1, 'Asia/Ho_Chi_Minh', "
                    '\'{"wait_for_selector": ".price"}\', NULL, NULL, '
                    "'2026-10-20 00:00:00', '2026-10-20 00:00:00')"
                )
            )
        assert "wait_for_api" not in await _columns(db, "supermarkets")

        await db.create_all()
        await db.create_all()  # running again on every start is harmless

        assert {"wait_for_api", "api_url_contains"} <= await _columns(db, "supermarkets")
        old = await supermarkets.get_supermarket(db, "bach-hoa-xanh")
        assert (old.wait_for_api, old.api_url_contains) == (False, None)
        assert old.cron_expression == "0 10 * * 2,6"
        assert old.crawl_config == {"wait_for_selector": ".price"}

        on = await supermarkets.update_supermarket(
            db, old.id, wait_for_api=True, api_url_contains="/gw/Product/GetProductDetail"
        )
        assert on.wait_for_api is True
        # An older image (without these fields in its model) can still add supermarkets.
        async with db.engine.begin() as conn:
            await conn.execute(
                text(
                    "INSERT INTO supermarkets (id, name, base_url, cron_enabled, timezone, "
                    "crawl_config, created_at, updated_at) VALUES ('lotte', 'Lotte', "
                    "'https://www.lottemart.vn', 0, 'Asia/Ho_Chi_Minh', '{}', "
                    "'2026-11-01 00:00:00', '2026-11-01 00:00:00')"
                )
            )
        assert (await supermarkets.get_supermarket(db, "lotte")).wait_for_api is False
    finally:
        await db.dispose()
