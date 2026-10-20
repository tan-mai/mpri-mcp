"""Smoke-test a running mpri-mcp server end to end, the way Claude uses it.

Signs in to the dashboard, registers a throwaway OAuth client, approves it, exchanges the
code (PKCE), then calls MCP tools with the official MCP client. Read-only: it lists tools
and supermarkets, then revokes its own token.

    uv run python scripts/smoke_test.py https://mpri.example.com tan
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import getpass
import hashlib
import re
import secrets
import sys
from urllib.parse import parse_qs, urlparse

import httpx2
from mcp import Client
from mcp.client.streamable_http import streamable_http_client

REDIRECT = "http://localhost:53682/callback"  # never actually visited


def csrf(html: str) -> str:
    match = re.search(r'name="csrf_token" value="([^"]+)"', html)
    if not match:
        raise SystemExit("Could not find a CSRF token — is this an mpri-mcp server?")
    return match.group(1)


def step(message: str) -> None:
    print(f"  ✓ {message}")


async def run(base: str, username: str, password: str) -> None:
    async with httpx2.AsyncClient(base_url=base, follow_redirects=False, timeout=30) as web:
        health = await web.get("/health")
        health.raise_for_status()
        step(f"health: {health.json()}")

        resp = await web.get("/mcp")
        if resp.status_code != 401 or "resource_metadata" not in resp.headers.get(
            "www-authenticate", ""
        ):
            raise SystemExit(
                f"/mcp should answer 401 with resource_metadata, got {resp.status_code}"
            )
        step("/mcp asks for OAuth (401 + resource_metadata)")

        prm = (await web.get("/.well-known/oauth-protected-resource/mcp")).json()
        meta = (await web.get("/.well-known/oauth-authorization-server")).json()
        if prm["resource"].rstrip("/") != f"{base}/mcp":
            raise SystemExit(
                f"Protected resource is {prm['resource']} but you connect to {base}/mcp — "
                "check PUBLIC_BASE_URL."
            )
        if meta["issuer"] != prm["authorization_servers"][0]:
            raise SystemExit(
                f"Issuer mismatch: metadata says {meta['issuer']!r}, protected resource says "
                f"{prm['authorization_servers'][0]!r}. Clients compare these exactly."
            )
        step(f"discovery: issuer {meta['issuer']}")

        page = await web.get("/admin/login")
        resp = await web.post(
            "/admin/login",
            data={
                "csrf_token": csrf(page.text),
                "username": username,
                "password": password,
                "next": "/admin",
            },
        )
        if resp.status_code != 303:
            raise SystemExit(f"Login failed (HTTP {resp.status_code}).")
        step("signed in to the dashboard")

        reg = await web.post(
            "/register",
            json={
                "redirect_uris": [REDIRECT],
                "client_name": "mpri smoke test",
                "token_endpoint_auth_method": "none",
            },
        )
        reg.raise_for_status()
        client_id = reg.json()["client_id"]
        step(f"registered OAuth client {client_id}")

        verifier = secrets.token_urlsafe(48)
        challenge = (
            base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest())
            .decode()
            .rstrip("=")
        )
        resp = await web.get(
            "/authorize",
            params={
                "response_type": "code",
                "client_id": client_id,
                "redirect_uri": REDIRECT,
                "code_challenge": challenge,
                "code_challenge_method": "S256",
                "state": "smoke",
                "resource": f"{base}/mcp",
            },
        )
        consent_url = resp.headers["location"]
        consent = await web.get(consent_url)
        req = parse_qs(urlparse(consent_url).query)["req"][0]
        resp = await web.post(
            "/oauth/consent",
            data={"csrf_token": csrf(consent.text), "req": req, "decision": "allow"},
        )
        code = parse_qs(urlparse(resp.headers["location"]).query)["code"][0]
        tokens = (
            await web.post(
                "/token",
                data={
                    "grant_type": "authorization_code",
                    "code": code,
                    "code_verifier": verifier,
                    "redirect_uri": REDIRECT,
                    "client_id": client_id,
                },
            )
        ).json()
        if "access_token" not in tokens:
            raise SystemExit(f"Token exchange failed: {tokens}")
        step("consent approved, tokens issued")

        http = httpx2.AsyncClient(
            headers={"Authorization": f"Bearer {tokens['access_token']}"}, timeout=60
        )
        async with Client(streamable_http_client(f"{base}/mcp", http_client=http)) as mcp:
            tools = [t.name for t in (await mcp.list_tools()).tools]
            step(f"MCP tools ({len(tools)}): {', '.join(tools)}")
            result = await mcp.call_tool("get_supermarkets", {})
            markets = result.structured_content["supermarkets"]
            step(f"get_supermarkets: {len(markets)} supermarket(s)")
        await http.aclose()

        await web.post("/revoke", data={"token": tokens["refresh_token"], "client_id": client_id})
        step("revoked the smoke-test tokens (the client stays listed under Connected apps)")


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("base_url", help="e.g. https://mpri.example.com")
    parser.add_argument("username", help="admin username")
    parser.add_argument(
        "--password-stdin", action="store_true", help="read the password from stdin"
    )
    args = parser.parse_args()
    if args.password_stdin:
        password = sys.stdin.readline().rstrip("\r\n")
    else:
        password = getpass.getpass(f"Password for {args.username}: ")
    print(f"Testing {args.base_url}")
    asyncio.run(run(args.base_url.rstrip("/"), args.username, password))
    print("All good — add the connector in claude.ai.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
