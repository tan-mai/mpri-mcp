import re

import pytest
from fastapi.testclient import TestClient

from mpri_mcp.app import create_app

GOOD_PASSWORD = "correct horse battery"


def csrf(html: str) -> str:
    match = re.search(r'name="csrf_token" value="([^"]+)"', html)
    assert match, "no CSRF token in page"
    return match.group(1)


@pytest.fixture
def client(settings):
    with TestClient(create_app(settings), follow_redirects=False) as c:
        yield c


def do_setup(client: TestClient, token: str = "test-setup-token", **overrides) -> object:
    page = client.get("/admin/setup")
    data = {
        "csrf_token": csrf(page.text),
        "setup_token": token,
        "username": "tan",
        "password": GOOD_PASSWORD,
        "confirm": GOOD_PASSWORD,
        **overrides,
    }
    return client.post("/admin/setup", data=data)


def login(client: TestClient, password: str = GOOD_PASSWORD, next_url: str = "/admin"):
    page = client.get("/admin/login")
    return client.post(
        "/admin/login",
        data={
            "csrf_token": csrf(page.text),
            "username": "tan",
            "password": password,
            "next": next_url,
        },
    )


def test_fresh_install_redirects_to_setup(client):
    resp = client.get("/admin")
    assert resp.status_code == 303 and resp.headers["location"] == "/admin/setup"
    assert client.get("/admin/login").headers["location"] == "/admin/setup"
    page = client.get("/admin/setup")
    assert page.status_code == 200 and "Setup token" in page.text


def test_setup_requires_csrf(client):
    resp = client.post(
        "/admin/setup",
        data={
            "setup_token": "test-setup-token",
            "username": "tan",
            "password": GOOD_PASSWORD,
            "confirm": GOOD_PASSWORD,
        },
    )
    assert resp.status_code == 403


def test_setup_rejects_wrong_token_and_weak_password(client):
    resp = do_setup(client, token="wrong")
    assert resp.status_code == 400 and "setup token is wrong" in resp.text
    resp = do_setup(client, password="short", confirm="short")
    assert resp.status_code == 400 and "at least 12" in resp.text
    resp = do_setup(client, confirm="something else entirely")
    assert resp.status_code == 400 and "do not match" in resp.text
    resp = do_setup(client, username="no spaces allowed")
    assert resp.status_code == 400


def test_setup_once_then_dashboard(client):
    resp = do_setup(client)
    assert resp.status_code == 303 and resp.headers["location"] == "/admin"
    page = client.get("/admin")
    assert page.status_code == 200 and "<h2>Overview</h2>" in page.text
    assert ">tan</a>" in page.text  # signed-in admin shown in the nav
    assert "admin account is ready" in page.text  # flash message
    assert client.get("/admin/setup").status_code == 404
    assert client.post("/admin/setup", data={}).status_code == 404


def test_setup_token_attempts_are_rate_limited(client):
    for _ in range(5):
        do_setup(client, token="wrong")
    resp = do_setup(client)  # even the right token is refused now
    assert resp.status_code == 400 and "Too many attempts" in resp.text


def test_logout_and_login_flow(client):
    do_setup(client)
    page = client.get("/admin")
    client.post("/admin/logout", data={"csrf_token": csrf(page.text)})
    resp = client.get("/admin/account?tab=x")
    assert resp.status_code == 303
    assert resp.headers["location"] == "/admin/login?next=%2Fadmin%2Faccount%3Ftab%3Dx"

    assert login(client, password="wrong password!!").status_code == 401
    resp = login(client, next_url="/admin/account")
    assert resp.status_code == 303 and resp.headers["location"] == "/admin/account"


@pytest.mark.parametrize("evil", ["https://evil.example/", "//evil.example", "javascript:alert(1)"])
def test_login_rejects_open_redirect(client, evil):
    do_setup(client)
    page = client.get("/admin")
    client.post("/admin/logout", data={"csrf_token": csrf(page.text)})
    resp = login(client, next_url=evil)
    assert resp.headers["location"] == "/admin"


def test_login_lockout(client):
    do_setup(client)
    page = client.get("/admin")
    client.post("/admin/logout", data={"csrf_token": csrf(page.text)})
    for _ in range(5):
        assert login(client, password="nope nope nope").status_code == 401
    resp = login(client)
    assert resp.status_code == 429


def test_password_change_signs_out_other_sessions(settings):
    app = create_app(settings)
    with TestClient(app, follow_redirects=False) as a:
        b = TestClient(app, follow_redirects=False)  # second browser, same running app
        do_setup(a)
        assert login(b).status_code == 303
        assert b.get("/admin").status_code == 200

        page = a.get("/admin/account")
        new = "an even better passphrase"
        resp = a.post(
            "/admin/account/password",
            data={
                "csrf_token": csrf(page.text),
                "current": GOOD_PASSWORD,
                "password": new,
                "confirm": new,
            },
        )
        assert resp.status_code == 303
        assert a.get("/admin").status_code == 200  # this browser stays signed in
        assert b.get("/admin").status_code == 303  # the other one is out


def test_change_password_requires_current(client):
    do_setup(client)
    page = client.get("/admin/account")
    resp = client.post(
        "/admin/account/password",
        data={
            "csrf_token": csrf(page.text),
            "current": "wrong",
            "password": "x" * 20,
            "confirm": "x" * 20,
        },
    )
    assert resp.status_code == 400 and "Current password is wrong" in resp.text


def test_security_headers(client):
    resp = client.get("/admin/setup")
    assert "frame-ancestors 'none'" in resp.headers["content-security-policy"]
    assert resp.headers["x-frame-options"] == "DENY"
    assert resp.headers["x-content-type-options"] == "nosniff"
    assert "max-age" not in resp.headers.get("strict-transport-security", "")  # http in tests


def test_unknown_admin_page_renders_html_404(client):
    do_setup(client)
    resp = client.get("/admin/does-not-exist")
    assert resp.status_code == 404 and "<html" in resp.text


async def test_reset_password_bumps_session_version(db):
    from mpri_mcp.auth.admin import authenticate, create_admin, set_password

    user = await create_admin(db, "tan", GOOD_PASSWORD)
    updated = await set_password(db, username=None, password="brand new passphrase")
    assert updated.session_version == user.session_version + 1
    assert await authenticate(db, "tan", "brand new passphrase")
    assert await authenticate(db, "tan", GOOD_PASSWORD) is None
