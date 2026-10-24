from fastapi.testclient import TestClient

from mpri_mcp.app import create_app


def test_health(settings):
    with TestClient(create_app(settings)) as client:
        resp = client.get("/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] == "ok"
    assert body["commit"] == "dev"  # MPRI_COMMIT is only set inside the CI-built image
