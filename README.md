<!-- TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host -->
<p align="center"><img src="mpri-mcp/assets/static/avatar-192.png" width="96" alt=""></p>

# MPriMCP

[![CI](https://github.com/tan-mai/mpri-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/tan-mai/mpri-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

MPriMCP is a self-hosted [MCP](https://modelcontextprotocol.io) server that tracks product prices
at Vietnamese supermarkets (in VND). You tell your AI which supermarkets you shop at, which
products to watch and at what price you want to be told; the AI reads the prices with its browser
tool, MPriMCP stores them, compares supermarkets, keeps the price history and messages you on
Telegram or Discord when something drops below your target. The server itself never visits the
supermarket websites. It is one small binary with an embedded SQLite database.

> Hình/GIF onboarding: _(add a recording of `init` here)_

```text
You:  init
AI:   Bạn hay mua ở siêu thị nào? …  (adds Bách Hóa Xanh and WinMart, finds their search pages)
AI:   Bạn muốn theo dõi sản phẩm gì? …  (searches both sites, shows matching packs and prices)
AI:   Báo khi dưới bao nhiêu? … Telegram hay Discord? … Kiểm tra giá lúc nào?
You:  so sánh giá sữa TH ít đường 110ml
```

## Requirements

- An AI client that supports MCP (Claude Desktop, Claude Code, Codex, Kimi CLI, Gemini CLI, Cursor,
  VS Code, …).
- A browser tool for that client: Claude in Chrome, [Playwright MCP](https://github.com/microsoft/playwright-mcp),
  [Chrome DevTools MCP](https://github.com/ChromeDevTools/chrome-devtools-mcp), …

## Install

Download the archive for your OS from [GitHub Releases](https://github.com/tan-mai/mpri-mcp/releases)
(Linux x86_64/arm64, macOS Intel/Apple Silicon, Windows x86_64) and put `mprimcp` somewhere on your
`PATH`. Or build it:

```bash
cargo install --git https://github.com/tan-mai/mpri-mcp mprimcp
```

Data is stored in `~/.local/share/mprimcp` (Linux), `~/Library/Application Support/mprimcp` (macOS)
or `%APPDATA%\mprimcp` (Windows); set `MPRIMCP_DATA_DIR` to use another folder. The database starts
empty.

## Connect your AI client

MPriMCP runs as a local stdio server (`mprimcp stdio`). Add a browser MCP next to it if your client
has none built in.

**Claude Desktop** (`claude_desktop_config.json`), then enable Claude in Chrome:

```json
{ "mcpServers": { "mprimcp": { "command": "/path/to/mprimcp", "args": ["stdio"] } } }
```

**Claude Code**

```bash
claude mcp add mprimcp -- /path/to/mprimcp stdio
```

**Codex** (`~/.codex/config.toml`)

```toml
[mcp_servers.mprimcp]
command = "/path/to/mprimcp"
args = ["stdio"]

[mcp_servers.playwright]
command = "npx"
args = ["@playwright/mcp@latest"]
```

**Gemini CLI** (`~/.gemini/settings.json`), **Kimi CLI** and **Cursor** (`~/.cursor/mcp.json`) use the
same `mcpServers` shape:

```json
{
  "mcpServers": {
    "mprimcp": { "command": "/path/to/mprimcp", "args": ["stdio"] },
    "playwright": { "command": "npx", "args": ["@playwright/mcp@latest"] }
  }
}
```

**VS Code** (`.vscode/mcp.json`)

```json
{ "servers": { "mprimcp": { "type": "stdio", "command": "/path/to/mprimcp", "args": ["stdio"] } } }
```

Clients that ignore MCP server instructions (Codex, Kimi, Gemini CLI, …) should read
[`AGENTS.md`](AGENTS.md): it holds the same workflows.

## Get started: type `init`

Type `init` (or pick the `init` prompt). The AI runs the onboarding step by step: supermarkets,
products, price targets, notifications, schedule and the first price check. Run it again any time
to add or change things; steps already done are skipped.

Every day: *"chạy crawl"*, *"so sánh giá X"*, *"báo tôi khi Y dưới 70.000đ"*, *"món nào đang giảm giá
sâu?"*.

## Automation

Prices are read by the AI, so a scheduled price check is a scheduled AI task:

- **In your AI client** (recommended for desktop use): create a scheduled task with the prompt
  *"Call trigger_crawl_now for every MPriMCP supermarket, then run MPriMCP Workflow A until no pending
  tasks remain."* The computer must be on with the browser available.
- **On a server**: `MPRIMCP_CRON_ENABLED=true mprimcp serve` queues the crawl tasks on each
  supermarket's schedule (5-field cron, Vietnam time); your AI picks them up the next time it runs
  Workflow A. Schedules set with `setup_supermarket_cron_task` are always saved, also while the
  server cron is off.

## Telegram / Discord notifications

The server messages you when a saved price is below a target, and after every finished crawl with
where each tracked product is cheapest. Ask the AI to set it up (`set_notification_channel` sends a
test message right away) or use the dashboard.

- **Telegram:** talk to [@BotFather](https://t.me/BotFather), send `/newbot`, copy the token
  (`123456789:AA…`). Send any message to your new bot, then open
  `https://api.telegram.org/bot<TOKEN>/getUpdates` and copy `chat.id`.
- **Discord:** channel settings → Integrations → Webhooks → New Webhook → Copy Webhook URL.

Tokens are stored in the local database and are never shown in full (only `123456***xyz`).

## Dashboard

```bash
mprimcp dashboard          # prints http://127.0.0.1:8765/admin
```

The first start asks for a username and password (at least 12 characters, stored as an argon2id
hash). Screens: overview, supermarkets (schedule on/off), products and latest prices, prices waiting
for review, price alerts, notifications. Forgot the password: `mprimcp user set-password <username>`.
`stdio` mode needs no account.

## Self-host on a server

`mprimcp serve` exposes MCP over Streamable HTTP at `/mcp` (OAuth 2.1 with dynamic client
registration: your client opens the server's sign-in page), the dashboard and the optional cron.
Run it behind a reverse proxy with HTTPS and set:

| Variable | Purpose |
|---|---|
| `MPRIMCP_BIND` | Listen address (default `127.0.0.1:8765`) |
| `MPRIMCP_PUBLIC_URL` | Public origin, e.g. `https://mpri.example.com` (connector URL = `…/mcp`) |
| `MPRIMCP_ADMIN_USER`, `MPRIMCP_ADMIN_PASSWORD` | First account on non-interactive starts (Docker, systemd); ignored once an account exists |
| `MPRIMCP_CRON_ENABLED` | `true` to queue crawls on the supermarkets' schedules |
| `MPRIMCP_TRUST_PROXY` | `true` behind a proxy, so login limits see the client IP |
| `MPRIMCP_OAUTH_REDIRECT_URIS` | Extra allowed OAuth redirect URIs (Claude's and loopback are built in) |
| `MPRIMCP_DATA_DIR`, `RUST_LOG` | Data folder, log level |

Docker image, compose file with Caddy, and a systemd unit: [`mpri-mcp/deploy/`](mpri-mcp/deploy/)
([guide](mpri-mcp/deploy/VPS_SETUP.md)). All settings: [`mpri-mcp/.env.example`](mpri-mcp/.env.example).

```text
mprimcp stdio | serve | dashboard | import --from <sqlite-file> | user create | user set-password <username> | version
```

## FAQ and troubleshooting

- **The AI does not see the tools.** Check the path in the client config and run
  `/path/to/mprimcp version` in a terminal. Restart the client after editing its config.
- **"I have no browser tool".** Enable Claude in Chrome, or add Playwright MCP / Chrome DevTools MCP.
- **A price was flagged `needs_review`.** It moved more than 50% from the previous one; accept or
  reject it on the dashboard (Cần duyệt).
- **"database is locked".** `stdio` and `dashboard` can run at the same time (SQLite WAL); make sure
  no other program holds the file and that only one `serve` uses a data folder.
- **A site blocks your server.** Some supermarkets block datacenter IPs. Prices are read by your
  AI's browser, so this only matters when that browser runs on the server.

## Note

You are responsible for using the supermarkets' websites in line with their terms of use. MPriMCP
only stores what your AI reads.

## License

[MIT](LICENSE) © 2026 Tan Mai
