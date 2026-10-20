from __future__ import annotations

from collections.abc import AsyncIterator
from datetime import datetime
from pathlib import Path

import pytest

from mpri_mcp.config import Settings
from mpri_mcp.db import Database
from mpri_mcp.db.models import PriceRecord


@pytest.fixture
def settings(tmp_path: Path) -> Settings:
    return Settings(
        _env_file=None,
        data_dir=tmp_path / "data",
        public_base_url="http://localhost:8000",
        session_secret="test-session-secret",
        admin_setup_token="test-setup-token",
        ai_provider="fake",
        ai_model="fake-model",
    )


@pytest.fixture
async def db(settings: Settings) -> AsyncIterator[Database]:
    database = Database(settings.resolved_database_url)
    await database.create_all()
    yield database
    await database.dispose()


async def add_price(
    db: Database,
    url_id: int,
    current: int,
    original: int | None = None,
    member: int | None = None,
    at: datetime | None = None,
) -> None:
    async with db.session() as s:
        rec = PriceRecord(
            product_url_id=url_id,
            current_price=current,
            original_price=original,
            member_price=member,
        )
        if at is not None:
            rec.crawled_at = at
        s.add(rec)
