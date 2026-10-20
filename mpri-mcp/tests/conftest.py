from __future__ import annotations

from pathlib import Path

import pytest

from mpri_mcp.config import Settings


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
