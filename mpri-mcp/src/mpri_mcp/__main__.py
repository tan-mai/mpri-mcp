"""Command-line entry point: `mpri-mcp serve` (default) and maintenance/debug commands."""

from __future__ import annotations

import argparse
import asyncio
import sys

from mpri_mcp.config import get_settings
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
        # TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
        # The scheduler lives in this process: more than one worker would queue every crawl
        # twice and break the atomic task claim.
        workers=1,
    )
    return 0


# TM - #{ticket number} - MPriMCP Agent Crawl & Product Search
# (the crawler commands digest / ai-test / crawl were removed with the server crawler)


def _reset_password(args: argparse.Namespace) -> int:
    import getpass

    from mpri_mcp.auth.admin import set_password
    from mpri_mcp.auth.passwords import validate_new_password, validate_username
    from mpri_mcp.db import Database

    settings = get_settings()
    configure_logging("WARNING")
    if args.password_stdin:
        password = confirm = sys.stdin.readline().rstrip("\r\n")
    else:
        password = getpass.getpass("New admin password: ")
        confirm = getpass.getpass("Repeat password: ")
    username = args.username
    if username and (error := validate_username(username)):
        print(error, file=sys.stderr)
        return 1
    if error := validate_new_password(password, confirm, username or ""):
        print(error, file=sys.stderr)
        return 1

    async def run() -> int:
        db = Database(settings.resolved_database_url)
        await db.create_all()
        try:
            user = await set_password(
                db, username=username, password=password, create_if_missing=True
            )
        except LookupError:
            print("No admin account found. Pass --username to create one.", file=sys.stderr)
            return 1
        finally:
            await db.dispose()
        print(f"Password set for '{user.username}'. All sessions were signed out.")
        return 0

    return asyncio.run(run())


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mpri-mcp", description="Market Pricing MCP server")
    sub = parser.add_subparsers(dest="command")

    serve = sub.add_parser("serve", help="Run the HTTP server (default)")
    serve.add_argument("--host")
    serve.add_argument("--port", type=int)
    serve.set_defaults(func=_serve)

    admin = sub.add_parser("admin", help="Admin account maintenance")
    admin_sub = admin.add_subparsers(dest="admin_command", required=True)
    reset = admin_sub.add_parser(
        "reset-password", help="Set a new admin password (creates the admin if none exists)"
    )
    reset.add_argument("--username", help="Admin username (required to create one)")
    reset.add_argument("--password-stdin", action="store_true", help="Read password from stdin")
    reset.set_defaults(func=_reset_password)

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
