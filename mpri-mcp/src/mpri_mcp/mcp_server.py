"""The MCP server (resource server): tools behind OAuth bearer-token verification."""

from __future__ import annotations

from mcp.server import MCPServer
from mcp.server.auth.settings import AuthSettings
from mcp.server.transport_security import TransportSecuritySettings
from pydantic import AnyHttpUrl

from mpri_mcp import __version__
from mpri_mcp.auth.oauth_provider import DbTokenVerifier, SqliteOAuthProvider
from mpri_mcp.auth.oauth_routes import issuer_url
from mpri_mcp.config import Settings
from mpri_mcp.container import Container
from mpri_mcp.tools.analytics_tools import register_analytics_tools
from mpri_mcp.tools.common import ToolContext
from mpri_mcp.tools.setup_tools import register_setup_tools

INSTRUCTIONS = """\
Tracks product prices at Vietnamese supermarkets (prices in VND).
Typical setup: get_supermarkets -> add_supermarket -> setup_supermarket_cron_task \
(standard cron, Vietnam time) -> add_product_with_urls -> optionally trigger_crawl_now.
Analysis: get_product_price_comparison, get_price_history, get_deep_discount_items. \
Product names are matched fuzzily and without accents. A product has no price until its \
supermarket has been crawled at least once.
This server's messages, errors and dates are in English unless the user picked Vietnamese for \
this connection; if the user wants Vietnamese (tiếng Việt), call set_response_language("vi").
"""


def build_mcp_server(container: Container, provider: SqliteOAuthProvider) -> MCPServer:
    settings = container.settings
    mcp = MCPServer(
        name="mpri",
        title="MPRI price tracker",
        instructions=INSTRUCTIONS,
        version=__version__,
        token_verifier=DbTokenVerifier(provider),
        auth=AuthSettings(
            issuer_url=issuer_url(settings),
            resource_server_url=AnyHttpUrl(settings.mcp_resource_url),
            required_scopes=[settings.oauth_scope],
            validate_token_resource=True,
        ),
    )
    ctx = ToolContext(container)
    register_setup_tools(mcp, container, ctx)
    register_analytics_tools(mcp, container, ctx)
    return mcp


def transport_security(settings: Settings) -> TransportSecuritySettings:
    host = settings.public_host
    hostname = host.split(":")[0]
    scheme = "https" if settings.is_https else "http"
    return TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=[host, f"{hostname}:*", "localhost:*", "127.0.0.1:*", "[::1]:*"],
        allowed_origins=[
            f"{scheme}://{host}",
            "http://localhost:*",
            "http://127.0.0.1:*",
        ],
    )
