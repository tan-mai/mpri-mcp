"""MCP tools for configuration: supermarkets, schedules, products."""

from typing import Annotated, Any

from mcp.server import MCPServer
from mcp.types import ToolAnnotations
from pydantic import BaseModel, Field

from mpri_mcp.container import Container
from mpri_mcp.services import products, schedules, supermarkets
from mpri_mcp.services.products import UrlInput
from mpri_mcp.tools.common import Clock, drop_none, service_errors

_WRITE = ToolAnnotations(readOnlyHint=False, destructiveHint=False, openWorldHint=False)
_WRITE_IDEMPOTENT = ToolAnnotations(
    readOnlyHint=False, destructiveHint=False, idempotentHint=True, openWorldHint=False
)
_READ = ToolAnnotations(readOnlyHint=True, openWorldHint=False)


class ProductUrlInput(BaseModel):
    supermarket_id: str = Field(description="ID from get_supermarkets, e.g. 'winmart'.")
    url: str = Field(description="Full product page URL at that supermarket.")


def register_setup_tools(mcp: MCPServer, c: Container) -> None:
    clock = Clock(c.settings.scheduler_timezone)

    @mcp.tool(title="List supermarkets", annotations=_READ)
    @service_errors
    async def get_supermarkets() -> dict[str, Any]:
        """List every configured supermarket with its ID, crawl schedule (cron task) status,
        next run time, number of tracked product URLs, and the result of the last crawl."""
        summaries = await supermarkets.list_supermarkets(c.db)
        result = []
        for item in summaries:
            m, run = item.supermarket, item.last_run
            result.append(
                {
                    "id": m.id,
                    "name": m.name,
                    "base_url": m.base_url,
                    "schedule": {
                        "cron_expression": m.cron_expression,
                        "enabled": m.cron_enabled,
                        "timezone": m.timezone,
                        "next_run": clock.human(c.scheduler.next_run_time(m.id)),
                        "crawl_in_progress": c.scheduler.is_running(m.id),
                    },
                    "tracked_urls": item.active_url_count,
                    "failing_urls": item.failing_url_count,
                    "last_run": drop_none(
                        {
                            "status": run.status,
                            "trigger": run.trigger,
                            "started_at": clock.human(run.started_at),
                            "urls_ok": run.urls_ok,
                            "urls_failed": run.urls_failed,
                            "error": run.error,
                        }
                    )
                    if run
                    else None,
                }
            )
        return {"supermarkets": result, "timezone": clock.name}

    @mcp.tool(title="Add supermarket", annotations=_WRITE)
    @service_errors
    async def add_supermarket(
        name: Annotated[str, Field(description="Display name, e.g. 'Lotte Mart'.")],
        base_url: Annotated[str, Field(description="Website domain or URL, e.g. 'lottemart.vn'.")],
    ) -> dict[str, Any]:
        """Add a supermarket to track. Returns its ID, which the other tools need.
        After adding, set a crawl schedule with setup_supermarket_cron_task."""
        m = await supermarkets.add_supermarket(c.db, name, base_url)
        return {
            "id": m.id,
            "name": m.name,
            "base_url": m.base_url,
            "message": f"Added {m.name} with ID '{m.id}'. It has no crawl schedule yet.",
        }

    @mcp.tool(title="Set crawl schedule", annotations=_WRITE_IDEMPOTENT)
    @service_errors
    async def setup_supermarket_cron_task(
        supermarket_id: Annotated[str, Field(description="Supermarket ID from get_supermarkets.")],
        cron_expression: Annotated[
            str,
            Field(
                description="Standard 5-field cron (minute hour day month weekday) in Vietnam "
                "time (Asia/Ho_Chi_Minh). '0 3 * * *' = 03:00 daily; '30 6 * * 1-5' = 06:30 "
                "Mon–Fri; weekday 0 or 7 = Sunday. Macros like @daily also work."
            ),
        ],
    ) -> dict[str, Any]:
        """Create or replace the independent crawl schedule (cron task) for one supermarket.
        Takes effect immediately, no restart. Each supermarket has its own job, so a slow or
        failing site never delays the others. Also re-enables a disabled schedule."""
        r = await schedules.set_cron(c.db, c.scheduler, supermarket_id, cron_expression)
        return {
            "supermarket_id": r.supermarket.id,
            "supermarket_name": r.supermarket.name,
            "cron_expression": r.cron_expression,
            "timezone": r.timezone,
            "next_runs": [clock.human(t) for t in r.next_runs],
            "message": f"Crawl schedule for {r.supermarket.name} set to '{r.cron_expression}'.",
        }

    @mcp.tool(title="Pause crawl schedule", annotations=_WRITE_IDEMPOTENT)
    @service_errors
    async def disable_supermarket_cron_task(
        supermarket_id: Annotated[str, Field(description="Supermarket ID from get_supermarkets.")],
    ) -> dict[str, Any]:
        """Pause a supermarket's crawl schedule. The cron expression is kept; call
        setup_supermarket_cron_task again to resume."""
        m = await schedules.disable_cron(c.db, c.scheduler, supermarket_id)
        return {
            "supermarket_id": m.id,
            "cron_expression": m.cron_expression,
            "enabled": False,
            "message": f"Paused the crawl schedule for {m.name}.",
        }

    @mcp.tool(
        title="Crawl now",
        annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=False, openWorldHint=True),
    )
    @service_errors
    async def trigger_crawl_now(
        supermarket_id: Annotated[str, Field(description="Supermarket ID from get_supermarkets.")],
    ) -> dict[str, Any]:
        """Start crawling one supermarket's product pages right away, in the background.
        Returns immediately; check get_supermarkets (last_run) after a minute or two."""
        started = await schedules.trigger_now(c.db, c.scheduler, supermarket_id)
        return {
            "supermarket_id": supermarket_id,
            "started": started,
            "message": "Crawl started in the background."
            if started
            else "A crawl of this supermarket is already running.",
        }

    @mcp.tool(title="Track product", annotations=_WRITE_IDEMPOTENT)
    @service_errors
    async def add_product_with_urls(
        name: Annotated[
            str, Field(description="Product name, e.g. 'Thùng Sữa TH True Milk Ít Đường 110ml'.")
        ],
        urls: Annotated[
            list[ProductUrlInput],
            Field(description="The product's page at each supermarket (one URL per supermarket)."),
        ],
        category: Annotated[str | None, Field(description="Optional category, e.g. 'Sữa'.")] = None,
    ) -> dict[str, Any]:
        """Start tracking a product's price at one or more supermarkets. If a product with the
        same name (ignoring accents/case) already exists, the URLs are added to it or update
        its existing URL for that supermarket. Prices are collected on each supermarket's
        crawl schedule (or use trigger_crawl_now)."""
        r = await products.add_product_with_urls(
            c.db, name, category, [UrlInput(u.supermarket_id, u.url) for u in urls]
        )
        ok = [u for u in r.urls if u.status != "error"]
        return {
            "product_id": r.product.id,
            "name": r.product.name,
            "category": r.product.category,
            "created": r.created,
            "urls": [
                drop_none(
                    {
                        "supermarket_id": u.supermarket_id,
                        "url": u.url,
                        "status": u.status,
                        "error": u.message,
                        "warning": u.warning,
                    }
                )
                for u in r.urls
            ],
            "message": (
                f"{'Now tracking' if r.created else 'Updated'} '{r.product.name}' at "
                f"{len(ok)} supermarket(s)."
            ),
        }
