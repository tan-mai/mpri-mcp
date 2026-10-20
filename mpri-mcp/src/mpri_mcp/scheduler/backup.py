"""Online SQLite backups using SQLite's backup API (safe while the app is writing)."""

from __future__ import annotations

import asyncio
import logging
import sqlite3
from pathlib import Path

from mpri_mcp.utils.time import utcnow

log = logging.getLogger(__name__)


def _backup(db_path: Path, backup_dir: Path, keep: int) -> Path:
    backup_dir.mkdir(parents=True, exist_ok=True)
    dest = backup_dir / f"mpri-{utcnow():%Y%m%d-%H%M%S}.db"
    src = sqlite3.connect(db_path)
    dst = sqlite3.connect(dest)
    try:
        with dst:
            src.backup(dst)
    finally:
        dst.close()
        src.close()
    for old in sorted(backup_dir.glob("mpri-*.db"))[:-keep]:
        old.unlink(missing_ok=True)
    return dest


async def backup_sqlite(db_path: Path, backup_dir: Path, keep: int = 7) -> Path:
    dest = await asyncio.to_thread(_backup, db_path, backup_dir, max(1, keep))
    log.info("SQLite backup written to %s", dest)
    return dest
