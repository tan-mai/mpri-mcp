"""MCP tools for the agent crawl: Claude claims crawl tasks, reads the prices in the browser
and sends them back."""

# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search

from typing import Annotated, Any, Literal

from mcp.server import MCPServer
from mcp.types import ToolAnnotations
from pydantic import BaseModel, Field

from mpri_mcp.container import Container
from mpri_mcp.services import agent_tasks
from mpri_mcp.services.agent_tasks import PriceResult
from mpri_mcp.tools.common import Clock, drop_none, service_errors

_WRITE = ToolAnnotations(readOnlyHint=False, destructiveHint=False, openWorldHint=False)


class PriceResultInput(BaseModel):
    task_id: str = Field(description="task_id from get_pending_crawl_tasks.")
    status: Literal["ok", "out_of_stock", "not_found", "error"] = Field(
        description="ok = price read; out_of_stock = page says sold out / 'hết hàng'; "
        "not_found = page is gone (404) or shows a different product; error = the page did "
        "not load or no price could be read (the task is retried later)."
    )
    price: int | None = Field(
        None,
        description="Required when status is ok: what a regular customer pays now for this "
        "exact item, integer VND ('45.900đ' -> 45900).",
    )
    original_price: int | None = Field(
        None, description="Struck-through / pre-discount price, if shown. Must be >= price."
    )
    member_price: int | None = Field(
        None, description="Separate member/loyalty-card price, if shown."
    )
    raw_text: str | None = Field(
        None, description="Short page text the price was read from (max 500 characters)."
    )
    note: str | None = Field(None, description="Reason for an error, or a remark (max 300).")


def register_agent_tools(mcp: MCPServer, c: Container) -> None:
    clock = Clock(c.settings.scheduler_timezone)

    @mcp.tool(title="Get crawl tasks", annotations=_WRITE)
    @service_errors
    async def get_pending_crawl_tasks(
        supermarket_id: Annotated[
            str | None, Field(description="Only this supermarket (optional).")
        ] = None,
        limit: Annotated[int, Field(description="How many tasks to take (1-50).")] = 20,
    ) -> dict[str, Any]:
        """Take the oldest pending crawl tasks and reserve them for you until
        lease_expires_at. For each task, open its url in the browser, read the price of that
        product only, and send the result with submit_price_results BEFORE lease_expires_at;
        after that the task goes back to the queue. remaining_pending is how many tasks are
        still waiting after these."""
        r = await agent_tasks.claim_tasks(c.db, c.settings, supermarket_id, limit)
        return {
            "tasks": [
                drop_none(
                    {
                        "task_id": t.task.id,
                        "product_name": t.product_name,
                        "category": t.category,
                        "supermarket_id": t.task.supermarket_id,
                        "supermarket_name": t.supermarket_name,
                        "url": t.task.url,
                        "last_price": t.last_price,
                        "last_original_price": t.last_original_price,
                        "last_checked_at": clock.human(t.last_checked_at),
                        "lease_expires_at": clock.human(t.task.lease_expires_at),
                    }
                )
                for t in r.tasks
            ],
            "count": len(r.tasks),
            "remaining_pending": r.remaining_pending,
            "currency": "VND",
            "timezone": clock.name,
        }

    @mcp.tool(title="Submit prices", annotations=_WRITE)
    @service_errors
    async def submit_price_results(
        results: Annotated[
            list[PriceResultInput],
            Field(description="One result per claimed task, at most 50 per call."),
        ],
    ) -> dict[str, Any]:
        """Send the prices you read for tasks from get_pending_crawl_tasks. Each result is
        checked on its own: invalid ones are listed in rejected with the reason (fix and
        send them again); the rest are saved. A price more than 50% away from the last one
        is saved but flagged needs_review. Never guess a price. The server itself messages
        the user about alerts_triggered and, once nothing is pending, a summary of where
        each product is cheapest (summary_sent_to): do not send those again."""
        r = await agent_tasks.submit_results(
            c.db,
            c.settings,
            [
                PriceResult(
                    task_id=i.task_id,
                    status=i.status,
                    price=i.price,
                    original_price=i.original_price,
                    member_price=i.member_price,
                    raw_text=i.raw_text,
                    note=i.note,
                )
                for i in results
            ],
        )
        # TM - #{ticket number} - MPriMCP Price Alerts & Notifications
        sent = await c.notifier.after_submit(r.saved, r.queue_empty, r.accepted)
        notify_note = None
        if (sent.alerts or (r.queue_empty and r.accepted)) and not sent.channels:
            notify_note = (
                "No Telegram/Discord channel is set up (dashboard → Notifications), so the "
                "user was not messaged: tell them in your summary."
            )
        return drop_none(
            {
                "accepted": r.accepted,
                "alerts_triggered": [
                    {
                        "product_name": h.product_name,
                        "target_price": h.target_price,
                        "prices": [
                            {"supermarket_id": p.supermarket_id, "price": p.price} for p in h.prices
                        ],
                    }
                    for h in sent.alerts
                ]
                or None,
                "summary_sent_to": sent.summary_sent_to or None,
                "notifications": notify_note,
                "requeued": r.requeued or None,
                "flagged": [
                    {
                        "task_id": f.task_id,
                        "product_name": f.product_name,
                        "supermarket_id": f.supermarket_id,
                        "price": f.price,
                        "last_price": f.last_price,
                        "change_pct": f.change_pct,
                    }
                    for f in r.flagged
                ],
                "rejected": [{"task_id": x.task_id, "reason": x.reason} for x in r.rejected],
                "warnings": r.warnings or None,
                "remaining_pending": r.remaining_pending,
            }
        )
