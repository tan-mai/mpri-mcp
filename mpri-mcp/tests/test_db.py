"""Database startup: a new database gets every table; a database left by the server-crawler
release is backed up and reset once, keeping the admin account and OAuth connections."""

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search

import sqlite3

import pytest
from sqlalchemy import func, inspect, select, text

from mpri_mcp.db import Database
from mpri_mcp.db.models import (
    AdminUser,
    Base,
    OAuthAuthorizationCode,
    OAuthClient,
    OAuthPendingAuthorization,
    OAuthToken,
    PriceRecord,
    Supermarket,
)
from mpri_mcp.services import supermarkets

# Business tables as the server-crawler release created them.
OLD_TABLES = [
    """CREATE TABLE supermarkets (
        id VARCHAR(64) NOT NULL PRIMARY KEY, name VARCHAR(200) NOT NULL UNIQUE,
        base_url VARCHAR(500) NOT NULL, cron_expression VARCHAR(100),
        cron_enabled BOOLEAN NOT NULL, timezone VARCHAR(64) NOT NULL,
        crawl_config JSON NOT NULL, ai_provider VARCHAR(50), ai_model VARCHAR(100),
        wait_for_api BOOLEAN DEFAULT 0 NOT NULL, api_url_contains VARCHAR(300),
        created_at DATETIME NOT NULL, updated_at DATETIME NOT NULL)""",
    """CREATE TABLE products (
        id INTEGER NOT NULL PRIMARY KEY, name VARCHAR(300) NOT NULL,
        normalized_name VARCHAR(300) NOT NULL UNIQUE, category VARCHAR(100),
        created_at DATETIME NOT NULL)""",
    """CREATE TABLE product_urls (
        id INTEGER NOT NULL PRIMARY KEY,
        product_id INTEGER NOT NULL REFERENCES products (id) ON DELETE CASCADE,
        supermarket_id VARCHAR(64) NOT NULL REFERENCES supermarkets (id) ON DELETE CASCADE,
        url VARCHAR(1000) NOT NULL, is_active BOOLEAN NOT NULL, last_crawled_at DATETIME,
        last_success_at DATETIME, last_error TEXT, consecutive_failures INTEGER NOT NULL,
        created_at DATETIME NOT NULL, UNIQUE (product_id, supermarket_id))""",
    """CREATE TABLE price_records (
        id INTEGER NOT NULL PRIMARY KEY,
        product_url_id INTEGER NOT NULL REFERENCES product_urls (id) ON DELETE CASCADE,
        current_price INTEGER NOT NULL, original_price INTEGER, member_price INTEGER,
        in_stock BOOLEAN, crawled_at DATETIME NOT NULL, provider VARCHAR(50),
        model VARCHAR(100), raw_extraction JSON)""",
    """CREATE TABLE crawl_runs (
        id INTEGER NOT NULL PRIMARY KEY,
        supermarket_id VARCHAR(64) NOT NULL REFERENCES supermarkets (id) ON DELETE CASCADE,
        "trigger" VARCHAR(20) NOT NULL, status VARCHAR(20) NOT NULL,
        started_at DATETIME NOT NULL, finished_at DATETIME, urls_total INTEGER NOT NULL,
        urls_ok INTEGER NOT NULL, urls_failed INTEGER NOT NULL, error TEXT)""",
    """CREATE TABLE crawl_run_items (
        id INTEGER NOT NULL PRIMARY KEY,
        run_id INTEGER NOT NULL REFERENCES crawl_runs (id) ON DELETE CASCADE,
        product_url_id INTEGER REFERENCES product_urls (id) ON DELETE SET NULL,
        url VARCHAR(1000) NOT NULL, status VARCHAR(20) NOT NULL, error TEXT,
        duration_ms INTEGER, provider VARCHAR(50), model VARCHAR(100), input_tokens INTEGER,
        output_tokens INTEGER, created_at DATETIME NOT NULL)""",
    """CREATE TABLE app_settings (
        "key" VARCHAR(100) NOT NULL PRIMARY KEY, value JSON, updated_at DATETIME NOT NULL)""",
]
NOW = "'2026-11-17 02:00:00'"
OLD_ROWS = [
    "INSERT INTO supermarkets VALUES ('coop-mark', 'Coop Mark', 'https://cooponline.vn', "
    "'0 9 * * 2,6', 1, 'Asia/Ho_Chi_Minh', '{}', NULL, NULL, 0, NULL, " + NOW + ", " + NOW + ")",
    "INSERT INTO products VALUES (1, 'Nước Satori 500 ml', 'nuoc satori 500 ml', 'Nước', "
    + NOW
    + ")",
    "INSERT INTO product_urls VALUES (1, 1, 'coop-mark', 'https://cooponline.vn/satori', 1, "
    + NOW
    + ", "
    + NOW
    + ", NULL, 0, "
    + NOW
    + ")",
    "INSERT INTO price_records VALUES (1, 1, 80000, 118000, NULL, 1, " + NOW + ", 'gemini', "
    "'gemini-2.5-flash-lite', '{}')",
    "INSERT INTO crawl_runs VALUES (1, 'coop-mark', 'cron', 'success', "
    + NOW
    + ", "
    + NOW
    + ", 1, 1, 0, NULL)",
    "INSERT INTO crawl_run_items VALUES (1, 1, 1, 'https://cooponline.vn/satori', 'ok', NULL, "
    "900, 'gemini', 'gemini-2.5-flash-lite', 100, 20, " + NOW + ")",
    "INSERT INTO app_settings VALUES ('ai', '{\"provider\": \"gemini\"}', " + NOW + ")",
]
AUTH_TABLES = [
    AdminUser.__table__,
    OAuthClient.__table__,
    OAuthPendingAuthorization.__table__,
    OAuthAuthorizationCode.__table__,
    OAuthToken.__table__,
]


async def _legacy_database(settings) -> Database:
    db = Database(settings.resolved_database_url)
    async with db.engine.begin() as conn:
        await conn.run_sync(lambda c: Base.metadata.create_all(c, tables=AUTH_TABLES))
        for statement in OLD_TABLES + OLD_ROWS:
            await conn.execute(text(statement))
    async with db.session() as s:
        s.add(AdminUser(username="tan", password_hash="argon2-hash"))
        s.add(OAuthClient(client_id="claude", client_name="Claude", info={}))
    return db


async def _count(db: Database, model) -> int:
    async with db.session() as s:
        return await s.scalar(select(func.count()).select_from(model)) or 0


async def test_new_database_gets_every_table(settings):
    db = Database(settings.resolved_database_url)
    try:
        await db.create_all()
        await db.create_all()  # running again on every start is harmless
        async with db.engine.connect() as conn:
            tables = await conn.run_sync(lambda c: set(inspect(c).get_table_names()))
            cols = await conn.run_sync(
                lambda c: {x["name"] for x in inspect(c).get_columns("price_records")}
            )
        assert {"crawl_tasks", "supermarkets", "price_records"} <= tables
        assert {"source", "needs_review", "raw_text"} <= cols
        assert not (settings.data_dir / "backups").exists()
    finally:
        await db.dispose()


async def test_server_crawler_database_is_backed_up_and_reset_once(settings):
    db = await _legacy_database(settings)
    try:
        await db.create_all()

        backups = list((settings.data_dir / "backups").glob("legacy-before-agent-crawl-*.db"))
        assert len(backups) == 1
        old = sqlite3.connect(backups[0])
        try:
            assert old.execute("SELECT count(*) FROM price_records").fetchone() == (1,)
            assert old.execute("SELECT crawl_config FROM supermarkets").fetchone() == ("{}",)
        finally:
            old.close()

        async with db.engine.connect() as conn:
            cols = await conn.run_sync(
                lambda c: {x["name"] for x in inspect(c).get_columns("supermarkets")}
            )
            tables = await conn.run_sync(lambda c: set(inspect(c).get_table_names()))
        assert "search_url_template" in cols and "crawl_config" not in cols
        assert "crawl_tasks" in tables
        assert await _count(db, Supermarket) == 0 and await _count(db, PriceRecord) == 0
        # Login and Claude's connection survive the reset.
        assert await _count(db, AdminUser) == 1 and await _count(db, OAuthClient) == 1

        await supermarkets.add_supermarket(db, "Coop Mark", "cooponline.vn")
        await db.create_all()  # next start: nothing to reset
        assert await _count(db, Supermarket) == 1
        assert len(list((settings.data_dir / "backups").glob("legacy-*.db"))) == 1
    finally:
        await db.dispose()


async def test_other_schema_mismatch_stops_the_server(settings):
    db = Database(settings.resolved_database_url)
    try:
        async with db.engine.begin() as conn:
            await conn.execute(text("CREATE TABLE products (id INTEGER PRIMARY KEY)"))
        with pytest.raises(RuntimeError, match="does not match this version.*normalized_name"):
            await db.create_all()
    finally:
        await db.dispose()
