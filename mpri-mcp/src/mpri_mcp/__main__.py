"""Command-line entry point: `mpri-mcp serve` (default) and maintenance/debug commands."""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from dataclasses import asdict
from typing import Any

from mpri_mcp.config import Settings, get_settings
from mpri_mcp.log import configure_logging


def _serve(args: argparse.Namespace) -> int:
    import uvicorn

    settings = get_settings()
    uvicorn.run(
        "mpri_mcp.app:create_app",
        factory=True,
        host=args.host or settings.host,
        port=args.port or settings.port,
        proxy_headers=True,
        forwarded_allow_ips=settings.forwarded_allow_ips,
        log_config=None,
        # The scheduler lives in this process: more than one worker would run every crawl twice.
        workers=1,
    )
    return 0


def _print_json(data: Any) -> None:
    print(json.dumps(data, ensure_ascii=False, indent=2, default=str))


async def _with_container(settings: Settings, fn: Any) -> int:
    from mpri_mcp.container import Container

    container = Container(settings)
    await container.start(scheduler=False)
    try:
        return await fn(container)
    finally:
        await container.stop()


def _digest(args: argparse.Namespace) -> int:
    settings = get_settings()
    configure_logging("WARNING")

    async def run(c: Any) -> int:
        result = await c.runner.dry_run(args.url, supermarket_id=args.supermarket, extract=False)
        if not result.ok or result.digest is None:
            print(f"ERROR: {result.error}", file=sys.stderr)
            return 1
        if args.json:
            _print_json(asdict(result.digest))
        else:
            print(result.digest.to_prompt())
            print(f"\n--- {len(result.digest.to_prompt())} characters", file=sys.stderr)
        return 0

    return asyncio.run(_with_container(settings, run))


def _ai_test(args: argparse.Namespace) -> int:
    settings = get_settings()
    configure_logging("WARNING")

    async def run(c: Any) -> int:
        result = await c.runner.dry_run(
            args.url, supermarket_id=args.supermarket, provider=args.provider, model=args.model
        )
        out = asdict(result)
        out.pop("digest", None)
        _print_json(out)
        return 0 if result.ok else 1

    return asyncio.run(_with_container(settings, run))


def _crawl(args: argparse.Namespace) -> int:
    settings = get_settings()
    configure_logging(settings.log_level)

    async def run(c: Any) -> int:
        from sqlalchemy import select

        from mpri_mcp.db.models import CrawlRun, ProductUrl

        if args.dry_run:
            async with c.db.session() as s:
                urls = (
                    await s.scalars(
                        select(ProductUrl.url).where(
                            ProductUrl.supermarket_id == args.supermarket_id, ProductUrl.is_active
                        )
                    )
                ).all()
            if not urls:
                print(f"No active URLs for '{args.supermarket_id}'.", file=sys.stderr)
                return 1
            failed = 0
            for url in urls:
                result = await c.runner.dry_run(url, supermarket_id=args.supermarket_id)
                out = asdict(result)
                out.pop("digest", None)
                _print_json(out)
                failed += not result.ok
            return 1 if failed else 0
        run_id = await c.runner.crawl_supermarket(args.supermarket_id, trigger="manual")
        async with c.db.session() as s:
            run = await s.get(CrawlRun, run_id)
        print(
            f"Run #{run_id}: {run.status} — {run.urls_ok} ok, {run.urls_failed} failed"
            + (f" ({run.error})" if run.error else "")
        )
        return 0 if run.status == "success" else 1

    return asyncio.run(_with_container(settings, run))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mpri-mcp", description="Market Pricing MCP server")
    sub = parser.add_subparsers(dest="command")

    serve = sub.add_parser("serve", help="Run the HTTP server (default)")
    serve.add_argument("--host")
    serve.add_argument("--port", type=int)
    serve.set_defaults(func=_serve)

    digest = sub.add_parser("digest", help="Fetch a page and print what would be sent to the AI")
    digest.add_argument("url")
    digest.add_argument("--supermarket", help="Use this supermarket's crawl_config")
    digest.add_argument("--json", action="store_true", help="Print the digest as JSON")
    digest.set_defaults(func=_digest)

    ai_test = sub.add_parser("ai-test", help="Fetch + extract one URL without saving")
    ai_test.add_argument("url")
    ai_test.add_argument("--supermarket")
    ai_test.add_argument("--provider")
    ai_test.add_argument("--model")
    ai_test.set_defaults(func=_ai_test)

    crawl = sub.add_parser("crawl", help="Crawl one supermarket now")
    crawl.add_argument("supermarket_id")
    crawl.add_argument("--dry-run", action="store_true", help="Extract and print, don't save")
    crawl.set_defaults(func=_crawl)

    return parser


def main(argv: list[str] | None = None) -> int:
    # Vietnamese output on Windows consoles (cp1252) would otherwise crash print().
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    parser = build_parser()
    args = parser.parse_args(argv)
    if not getattr(args, "func", None):
        args = parser.parse_args(["serve", *(argv or [])])
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
