"""End-to-end OAuth 2.1 flow as Claude performs it, against the real ASGI app."""

import base64
import hashlib
import secrets
from urllib.parse import parse_qs, urlparse

import pytest
from fastapi.testclient import TestClient

from mpri_mcp.app import create_app
from mpri_mcp.auth.oauth_provider import redirect_uri_matches
from tests.test_admin_auth import csrf, do_setup

BASE = "http://localhost:8000"
CLAUDE_CALLBACK = "https://claude.ai/api/mcp/auth_callback"
MCP_HEADERS = {
    "Accept": "application/json, text/event-stream",
    "Content-Type": "application/json",
}


def pkce() -> tuple[str, str]:
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest())
    return verifier, challenge.decode().rstrip("=")


@pytest.fixture
def client(settings):
    with TestClient(create_app(settings), base_url=BASE, follow_redirects=False) as c:
        yield c


def register(client, redirect=CLAUDE_CALLBACK, method="client_secret_post") -> dict:
    resp = client.post(
        "/register",
        json={
            "redirect_uris": [redirect],
            "client_name": "Claude",
            "token_endpoint_auth_method": method,
            "grant_types": ["authorization_code", "refresh_token"],
        },
    )
    assert resp.status_code == 201, resp.text
    return resp.json()


def authorize(client, reg, challenge, redirect=CLAUDE_CALLBACK, **extra):
    params = {
        "response_type": "code",
        "client_id": reg["client_id"],
        "redirect_uri": redirect,
        "code_challenge": challenge,
        "code_challenge_method": "S256",
        "state": "state-123",
        "scope": "mpri",
        "resource": f"{BASE}/mcp",
        **extra,
    }
    return client.get("/authorize", params=params)


def approve(client, consent_location, decision="allow"):
    page = client.get(consent_location)
    assert page.status_code == 200, page.text
    req = parse_qs(urlparse(consent_location).query)["req"][0]
    return page, client.post(
        "/oauth/consent",
        data={"csrf_token": csrf(page.text), "req": req, "decision": decision},
    )


def exchange(client, reg, code, verifier, redirect=CLAUDE_CALLBACK):
    data = {
        "grant_type": "authorization_code",
        "code": code,
        "code_verifier": verifier,
        "redirect_uri": redirect,
        "client_id": reg["client_id"],
    }
    if reg.get("client_secret"):
        data["client_secret"] = reg["client_secret"]
    return client.post("/token", data=data)


def full_flow(client, redirect=CLAUDE_CALLBACK, method="client_secret_post"):
    reg = register(client, redirect, method)
    verifier, challenge = pkce()
    resp = authorize(client, reg, challenge, redirect)
    assert resp.status_code == 302, resp.text
    _, resp = approve(client, resp.headers["location"])
    assert resp.status_code == 303
    callback = urlparse(resp.headers["location"])
    query = parse_qs(callback.query)
    tokens = exchange(client, reg, query["code"][0], verifier, redirect)
    assert tokens.status_code == 200, tokens.text
    return reg, verifier, query, tokens.json()


def mcp_call(client, token, method="tools/list", params=None, req_id=1):
    headers = dict(MCP_HEADERS)
    if token:
        headers["Authorization"] = f"Bearer {token}"
    body = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params or {}}
    return client.post("/mcp", json=body, headers=headers)


def initialize(client, token):
    return mcp_call(
        client,
        token,
        "initialize",
        {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"},
        },
    )


# --------------------------------------------------------------------------- discovery


def test_unauthenticated_mcp_gets_401_with_metadata_pointer(client):
    resp = initialize(client, None)
    assert resp.status_code == 401
    www = resp.headers["www-authenticate"]
    assert f'resource_metadata="{BASE}/.well-known/oauth-protected-resource/mcp"' in www


def test_discovery_documents(client):
    prm = client.get("/.well-known/oauth-protected-resource/mcp").json()
    assert prm["resource"] == f"{BASE}/mcp"
    assert prm["authorization_servers"] == [BASE]  # exact: issuers compare as strings
    assert prm["scopes_supported"] == ["mpri"]

    meta = client.get("/.well-known/oauth-authorization-server").json()
    assert meta["issuer"] == BASE
    assert meta["registration_endpoint"] == f"{BASE}/register"
    assert meta["code_challenge_methods_supported"] == ["S256"]
    assert "none" in meta["token_endpoint_auth_methods_supported"]
    assert meta["authorization_response_iss_parameter_supported"] is True


def test_registration_rejects_unknown_redirects(client):
    resp = client.post("/register", json={"redirect_uris": ["https://evil.example/cb"]})
    assert resp.status_code == 400
    assert resp.json()["error"] == "invalid_redirect_uri"


# --------------------------------------------------------------------------- the flow


def test_full_flow_and_mcp_access(client):
    do_setup(client)
    reg, _, query, tokens = full_flow(client)
    assert query["state"] == ["state-123"]
    assert query["iss"] == [BASE]
    assert tokens["token_type"] == "Bearer" and tokens["refresh_token"]
    assert tokens["scope"] == "mpri"

    init = initialize(client, tokens["access_token"])
    assert init.status_code == 200, init.text
    assert init.json()["result"]["serverInfo"]["name"] == "mpri"

    listed = mcp_call(client, tokens["access_token"])
    assert listed.status_code == 200, listed.text
    names = {t["name"] for t in listed.json()["result"]["tools"]}
    assert "get_product_price_comparison" in names

    # Garbage and missing tokens are refused.
    assert mcp_call(client, "mpri_at_nope").status_code == 401


def test_consent_requires_login_and_deny_returns_error(client):
    do_setup(client)
    reg = register(client)
    _, challenge = pkce()
    location = authorize(client, reg, challenge).headers["location"]

    page = client.get("/admin")
    client.post("/admin/logout", data={"csrf_token": csrf(page.text)})
    resp = client.get(location)
    assert resp.status_code == 303 and resp.headers["location"].startswith("/admin/login?next=")

    from tests.test_admin_auth import login

    login(client)
    page, resp = approve(client, location, decision="deny")
    assert "claude.ai" in page.text  # redirect host shown prominently
    query = parse_qs(urlparse(resp.headers["location"]).query)
    assert query["error"] == ["access_denied"] and query["state"] == ["state-123"]


def test_code_is_single_use_and_replay_revokes_tokens(client):
    do_setup(client)
    reg, verifier, query, tokens = full_flow(client)
    replay = exchange(client, reg, query["code"][0], verifier)
    assert replay.status_code == 400 and replay.json()["error"] == "invalid_grant"
    assert initialize(client, tokens["access_token"]).status_code == 401


def test_wrong_pkce_verifier(client):
    do_setup(client)
    reg = register(client)
    _, challenge = pkce()
    location = authorize(client, reg, challenge).headers["location"]
    _, resp = approve(client, location)
    code = parse_qs(urlparse(resp.headers["location"]).query)["code"][0]
    bad = exchange(client, reg, code, "not-the-verifier")
    assert bad.status_code == 400 and bad.json()["error"] == "invalid_grant"


def test_refresh_rotation_and_reuse_detection(client):
    do_setup(client)
    reg, _, _, tokens = full_flow(client)
    form = {
        "grant_type": "refresh_token",
        "refresh_token": tokens["refresh_token"],
        "client_id": reg["client_id"],
        "client_secret": reg["client_secret"],
    }
    rotated = client.post("/token", data=form)
    assert rotated.status_code == 200
    new = rotated.json()
    assert new["refresh_token"] != tokens["refresh_token"]
    assert initialize(client, new["access_token"]).status_code == 200

    reuse = client.post("/token", data=form)  # the old refresh token again
    assert reuse.status_code == 400 and reuse.json()["error"] == "invalid_grant"
    # Theft assumed: the whole family, including the new access token, is revoked.
    assert initialize(client, new["access_token"]).status_code == 401


def test_revocation_endpoint(client):
    do_setup(client)
    reg, _, _, tokens = full_flow(client)
    resp = client.post(
        "/revoke",
        data={
            "token": tokens["access_token"],
            "client_id": reg["client_id"],
            "client_secret": reg["client_secret"],
        },
    )
    assert resp.status_code == 200
    assert initialize(client, tokens["access_token"]).status_code == 401


def test_public_client_with_loopback_any_port(client):
    """Claude Code / MCP Inspector style: no secret, loopback redirect on a new port."""
    do_setup(client)
    reg = register(client, "http://localhost:3118/callback", method="none")
    assert reg.get("client_secret") is None
    verifier, challenge = pkce()
    resp = authorize(client, reg, challenge, redirect="http://localhost:50123/callback")
    assert resp.status_code == 302, resp.text
    page, resp = approve(client, resp.headers["location"])
    assert "your own computer" in page.text
    code = parse_qs(urlparse(resp.headers["location"]).query)["code"][0]
    tokens = exchange(client, reg, code, verifier, redirect="http://localhost:50123/callback")
    assert tokens.status_code == 200


def test_resource_mismatch_is_rejected(client):
    do_setup(client)
    reg = register(client)
    _, challenge = pkce()
    resp = authorize(client, reg, challenge, resource="https://other.example/mcp")
    query = parse_qs(urlparse(resp.headers["location"]).query)
    assert query["error"] == ["invalid_target"]


def test_unregistered_redirect_at_authorize_is_not_followed(client):
    reg = register(client)
    _, challenge = pkce()
    resp = authorize(client, reg, challenge, redirect="https://claude.ai/other")
    assert resp.status_code == 400  # error shown directly, never redirected


async def test_client_revocation_blocks_tokens(settings):
    app = create_app(settings)
    with TestClient(app, base_url=BASE, follow_redirects=False) as c:
        do_setup(c)
        reg, _, _, tokens = full_flow(c)
        await app.state.mpri.oauth.revoke_client(reg["client_id"])
        assert initialize(c, tokens["access_token"]).status_code == 401


@pytest.mark.parametrize(
    ("registered", "requested", "ok"),
    [
        (CLAUDE_CALLBACK, CLAUDE_CALLBACK, True),
        (CLAUDE_CALLBACK, CLAUDE_CALLBACK + "x", False),
        ("http://localhost:3118/callback", "http://localhost:9999/callback", True),
        ("http://127.0.0.1/callback", "http://127.0.0.1:4000/callback", True),
        ("http://localhost:3118/callback", "http://localhost:3118/other", False),
        ("http://localhost:3118/callback", "http://127.0.0.1:3118/callback", False),
        ("https://localhost:3118/cb", "https://localhost:9/cb", False),
    ],
)
def test_redirect_matching(registered, requested, ok):
    assert redirect_uri_matches(registered, requested) is ok
