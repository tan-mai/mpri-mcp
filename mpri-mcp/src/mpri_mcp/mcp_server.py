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
from mpri_mcp.tools.agent_tools import register_agent_tools
from mpri_mcp.tools.analytics_tools import register_analytics_tools
from mpri_mcp.tools.setup_tools import register_setup_tools

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
INSTRUCTIONS = """\
Tracks product prices at Vietnamese supermarkets (prices in VND).

SETUP
get_supermarkets -> add_supermarket -> setup_supermarket_cron_task (standard cron,
Vietnam time) -> add_product_with_urls -> optionally trigger_crawl_now.
Product names are matched fuzzily and without accents. A product has no price until
its supermarket has been crawled at least once.

ANALYSIS
get_product_price_comparison, get_price_history, get_deep_discount_items.
Prices flagged needs_review=true are unverified: mention the flag when you show them.

HOW PRICES ARE COLLECTED
The server never fetches web pages. A supermarket's schedule (and trigger_crawl_now)
only queues crawl tasks; YOU read the prices with the browser (Workflow A).

WORKFLOW A - AGENT CRAWL (when asked to run the crawl, or when pending_tasks > 0)
1. Call get_pending_crawl_tasks (optionally per supermarket).
2. For each task, open task.url in the browser (Claude in Chrome / Claude Browser).
   Read the price ONLY for the product on that page. Ignore related products,
   combo/bundle prices and "frequently bought together" blocks.
3. Record:
   - price: the price a regular customer pays now for this exact item.
   - original_price: the struck-through / pre-discount price, if shown.
   - member_price: a separate member/loyalty price, if shown.
   - raw_text: the short page text the price was read from.
   Use status out_of_stock if the page says sold out / "hết hàng",
   not_found if the page is gone or shows a different product,
   error if the page did not load or you could not read a price.
   Never guess a price. Prices are integers in VND (e.g. "45.900đ" -> 45900).
4. Call submit_price_results in batches of up to 20, before lease_expires_at.
5. Repeat until remaining_pending is 0. Then summarise: ok / flagged / failed
   per supermarket, and list any discounts you saw.

WORKFLOW B - ADD A PRODUCT BY SEARCH (when the user wants to track a product by name)
1. Call get_supermarkets, then get_search_urls(keyword) for the chosen supermarkets
   (default: all).
2. Open each search_url in the browser (if null, open base_url and use the site's
   search box). Collect the top 3-5 relevant results per supermarket:
   name, pack size/quantity, current price, product page URL.
3. Group results by IDENTICAL SPEC (brand, variant, volume/weight, pack count,
   e.g. "thùng 48 hộp 110ml" vs "lốc 4 hộp 110ml" are DIFFERENT products).
   Present the groups to the user with numbers, showing which supermarkets carry
   each one and their prices. Point out spec differences explicitly.
4. WAIT for the user's confirmation. Never save anything without it.
   The user may pick one or more groups, or exclude some supermarkets.
5. For each confirmed group, call add_product_with_urls once with:
   - name: standard form "<Brand> <Product> <Variant> <Size> <Pack>",
     e.g. "Sữa TH True Milk Ít Đường 110ml Thùng 48 Hộp".
   - category if known.
   - one product page URL per supermarket (never a search-results URL).
   Only put URLs of the same spec into one product.
6. Offer trigger_crawl_now for the affected supermarkets so prices appear.
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
    register_setup_tools(mcp, container)
    register_analytics_tools(mcp, container)
    # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
    register_agent_tools(mcp, container)
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
