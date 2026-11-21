"""MCP tools for price alerts (target prices the user wants to be told about)."""

# TM - #{ticket number} - MPriMCP Price Alerts & Notifications

from typing import Annotated, Any

from mcp.server import MCPServer
from mcp.types import ToolAnnotations
from pydantic import Field

from mpri_mcp.container import Container
from mpri_mcp.services import alerts
from mpri_mcp.tools.common import Clock, drop_none, service_errors

_READ = ToolAnnotations(readOnlyHint=True, openWorldHint=False)
_WRITE = ToolAnnotations(
    readOnlyHint=False, destructiveHint=False, idempotentHint=True, openWorldHint=False
)
_DELETE = ToolAnnotations(readOnlyHint=False, destructiveHint=True, openWorldHint=False)


def register_alert_tools(mcp: MCPServer, c: Container) -> None:
    clock = Clock(c.settings.scheduler_timezone)

    def view(v: alerts.AlertView) -> dict[str, Any]:
        best = v.best
        return drop_none(
            {
                "product_name": v.product.name,
                "target_price": v.alert.target_price,
                "supermarket_ids": v.alert.supermarket_ids,  # absent = every supermarket
                "below_target_now": v.below,
                "cheapest": drop_none(
                    {
                        "supermarket_id": best.supermarket_id,
                        "supermarket_name": best.supermarket_name,
                        "current_price": best.current_price,
                        "checked_at": clock.human(best.crawled_at),
                        "needs_review": best.needs_review or None,
                        "url": best.url,
                    }
                )
                if best
                else None,
            }
        )

    @mcp.tool(title="Set price alert", annotations=_WRITE)
    @service_errors
    async def set_price_alert(
        product_name: Annotated[
            str, Field(description="Tracked product; fuzzy, accent-insensitive.")
        ],
        target_price: Annotated[
            int,
            Field(description="Notify when the price is BELOW this, in VND (e.g. 70000)."),
        ],
        supermarket_ids: Annotated[
            list[str] | None,
            Field(description="Only these supermarkets (IDs from get_supermarkets); default all."),
        ] = None,
    ) -> dict[str, Any]:
        """Tell the user (Telegram / Discord, set up on the dashboard) as soon as a crawl
        saves a price of this product below target_price. One alert per product: calling
        again replaces it. Check product_name in the answer: it is the matched product."""
        v, created = await alerts.set_alert(c.db, product_name, target_price, supermarket_ids)
        return {
            **view(v),
            "created": created,
            "message": (
                f"{'Set' if created else 'Updated'} the alert: {v.product.name} below "
                f"{target_price:,} VND." + (" It is already below that now." if v.below else "")
            ),
        }

    @mcp.tool(title="List price alerts", annotations=_READ)
    @service_errors
    async def get_price_alerts() -> dict[str, Any]:
        """Every price alert with its target and the cheapest current price it covers."""
        found = await alerts.list_alerts(c.db)
        return {"count": len(found), "alerts": [view(v) for v in found], "currency": "VND"}

    @mcp.tool(title="Delete price alert", annotations=_DELETE)
    @service_errors
    async def delete_price_alert(
        product_name: Annotated[str, Field(description="Product whose alert to remove (fuzzy).")],
    ) -> dict[str, Any]:
        """Remove the price alert of a product (the product stays tracked)."""
        name = await alerts.delete_alert(c.db, product_name)
        return {"product_name": name, "deleted": True, "message": f"Removed the alert for {name}."}
