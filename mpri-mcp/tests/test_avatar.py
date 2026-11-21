"""The MPRI avatar: dashboard favicon/brand, root icon paths, and the MCP serverInfo icons."""

# TM - #{ticket number} - MPriMCP Avatar

from fastapi.testclient import TestClient
from mcp import Client

from mpri_mcp.app import create_app
from mpri_mcp.container import Container
from mpri_mcp.mcp_server import build_mcp_server


def test_icons_are_served_and_linked(settings):
    with TestClient(create_app(settings), follow_redirects=False) as c:
        ico = c.get("/favicon.ico")
        assert ico.status_code == 200 and ico.content[:4] == b"\x00\x00\x01\x00"
        touch = c.get("/apple-touch-icon.png")
        assert touch.status_code == 200 and touch.content[:4] == b"\x89PNG"
        for path in ("/static/avatar.svg", "/static/avatar-512.png", "/static/avatar-192.png"):
            assert c.get(path).status_code == 200, path
        page = c.get("/admin/setup").text
        assert 'href="/favicon.ico"' in page and 'src="/static/avatar.svg"' in page


async def test_mcp_server_info_carries_the_avatar(settings):
    container = Container(settings)
    await container.start(scheduler=False)
    try:
        async with Client(build_mcp_server(container, container.oauth)) as client:
            info = client.server_info
    finally:
        await container.stop()
    srcs = {i.src: i.mime_type for i in info.icons}
    assert srcs == {
        "http://localhost:8000/static/avatar-512.png": "image/png",
        "http://localhost:8000/static/avatar-192.png": "image/png",
        "http://localhost:8000/static/avatar.svg": "image/svg+xml",
    }
    assert info.website_url == "http://localhost:8000"
