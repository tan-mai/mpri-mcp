"""MCP tools for reading prices: comparison, deep discounts, history."""

from typing import Annotated, Any

from mcp.server import MCPServer
from mcp.types import ToolAnnotations
from pydantic import Field

from mpri_mcp.container import Container
from mpri_mcp.services import analytics
from mpri_mcp.services.analytics import MarketPrice
from mpri_mcp.tools.common import Clock, drop_none, service_errors

_READ = ToolAnnotations(readOnlyHint=True, openWorldHint=False)


def register_analytics_tools(mcp: MCPServer, c: Container) -> None:
    clock = Clock(c.settings.scheduler_timezone)

    def price_row(p: MarketPrice) -> dict[str, Any]:
        return drop_none(
            {
                "supermarket_id": p.supermarket_id,
                "supermarket_name": p.supermarket_name,
                "current_price": p.current_price,
                "original_price": p.original_price,
                "member_price": p.member_price,
                "discount_pct": p.discount_pct,
                "member_discount_pct": p.member_discount_pct,
                "in_stock": p.in_stock,
                "checked_at": clock.human(p.crawled_at),
                "stale": p.is_stale or None,
                "no_data_yet": p.current_price is None or None,
                "last_error": p.last_error,
                "url": p.url,
            }
        )

    @mcp.tool(title="Compare prices", annotations=_READ)
    @service_errors
    async def get_product_price_comparison(
        product_name: Annotated[
            str,
            Field(description="Product name; fuzzy and accent-insensitive ('sua th it duong')."),
        ],
    ) -> dict[str, Any]:
        """Latest price of a tracked product at every supermarket, cheapest first, with
        original (pre-discount) and member prices and discount percentages. If the name
        matches several products closely, other_matches lists them."""
        cmp = await analytics.price_comparison(c.db, product_name)
        return {
            "product": {
                "id": cmp.product.id,
                "name": cmp.product.name,
                "category": cmp.product.category,
            },
            "currency": "VND",
            "prices": [price_row(p) for p in cmp.prices],
            "cheapest": price_row(cmp.cheapest) if cmp.cheapest else None,
            "other_matches": [m.name for m in cmp.alternatives],
        }

    @mcp.tool(title="Deep discounts", annotations=_READ)
    @service_errors
    async def get_deep_discount_items(
        min_discount_percentage: Annotated[
            int, Field(description="Minimum discount vs. the original price, in percent (1–99).")
        ],
        days: Annotated[int, Field(description="Only prices checked within this many days.")] = 7,
        limit: Annotated[int, Field(description="Maximum number of items.")] = 50,
    ) -> dict[str, Any]:
        """Products whose latest price is discounted by at least the given percentage,
        deepest discount first."""
        items = await analytics.deep_discounts(c.db, min_discount_percentage, days, limit)
        return {
            "currency": "VND",
            "count": len(items),
            "items": [
                drop_none(
                    {
                        "product_name": i.product_name,
                        "category": i.category,
                        "supermarket_id": i.supermarket_id,
                        "supermarket_name": i.supermarket_name,
                        "current_price": i.current_price,
                        "original_price": i.original_price,
                        "discount_pct": i.discount_pct,
                        "member_price": i.member_price,
                        "member_discount_pct": i.member_discount_pct,
                        "checked_at": clock.human(i.crawled_at),
                        "url": i.url,
                    }
                )
                for i in items
            ],
        }

    @mcp.tool(title="Price history", annotations=_READ)
    @service_errors
    async def get_price_history(
        product_name: Annotated[
            str, Field(description="Product name; fuzzy and accent-insensitive.")
        ],
        days: Annotated[int, Field(description="How many days back (1–730).")] = 30,
        supermarket_id: Annotated[
            str | None, Field(description="Limit to one supermarket (optional).")
        ] = None,
    ) -> dict[str, Any]:
        """Daily price history of a product per supermarket (last price of each day) with
        min / max / average and how today's price compares with the period's lowest."""
        h = await analytics.price_history(
            c.db, product_name, days, supermarket_id, c.settings.scheduler_timezone
        )
        series = []
        for s in h.series:
            low, latest = s.min_point, s.latest
            series.append(
                {
                    "supermarket_id": s.supermarket_id,
                    "supermarket_name": s.supermarket_name,
                    "points": [
                        drop_none(
                            {
                                "date": p.day.isoformat(),
                                "current_price": p.current_price,
                                "original_price": p.original_price,
                                "member_price": p.member_price,
                            }
                        )
                        for p in s.points
                    ],
                    "stats": drop_none(
                        {
                            "min_price": low.current_price if low else None,
                            "min_date": low.day.isoformat() if low else None,
                            "max_price": s.max_price,
                            "avg_price": s.avg_price,
                            "latest_price": latest.current_price if latest else None,
                            "latest_vs_min_pct": round(
                                (latest.current_price - low.current_price)
                                / low.current_price
                                * 100,
                                1,
                            )
                            if latest and low
                            else None,
                        }
                    ),
                }
            )
        return {
            "product": {"id": h.product.id, "name": h.product.name},
            "days": h.days,
            "currency": "VND",
            "series": series,
            "other_matches": [m.name for m in h.alternatives],
        }
