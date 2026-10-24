# mpri-mcp — Market Pricing MCP server

Tracks product prices at Vietnamese supermarkets (MM Mega Market, Winmart, Co.op, Go!, …)
and gives Claude tools to manage the tracking and analyse prices, as a **remote MCP
connector** secured with OAuth 2.1.

- **9 MCP tools.** Add supermarkets, set each one's independent crawl schedule, track products by URL, crawl on demand, compare prices, find deep discounts, and read price history.
- **Per-supermarket cron jobs.** One schedule per market (APScheduler, standard cron, Vietnam time). A slow or broken site never holds up the others.
- **Crawler.** Playwright renders each product page. An AI model reads a compact digest of the page and returns `current_price`, `original_price` and `member_price` as JSON.
- **Pluggable AI.** Choose Gemini (Vertex AI or AI Studio), any OpenAI-compatible API, or Anthropic Claude, globally or per supermarket, with an optional fallback provider.
- **Admin dashboard.** The admin account is created on the first visit. Pages: overview, supermarkets, products with price charts, crawl runs, AI settings and usage, and connected apps.
- **Self-contained.** One process and one SQLite file. GitHub Actions publishes the Docker image to `ghcr.io/tan-mai/mpri-mcp`; the VPS runs it with Docker Compose and Caddy (automatic HTTPS).

Requirements: [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md). Design: [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md).
Deployment: **[deploy/VPS_SETUP.md](deploy/VPS_SETUP.md)**.

## Build and deploy, in short

1. `git push` to `main`. The [**Build and publish image**](../.github/workflows/docker-publish.yml)
   workflow runs lint and tests, builds the image, and pushes it to
   `ghcr.io/tan-mai/mpri-mcp` tagged `sha-<commit>` and `latest`. A `v1.2.3` git tag also
   publishes `:1.2.3`.
2. Copy the `MPRI_IMAGE=ghcr.io/tan-mai/mpri-mcp:sha-…` line from the run's summary into the
   VPS `.env`.
3. On the VPS: `docker compose pull && docker compose up -d`. To roll back, put an older tag in
   `.env` and run the same command.

`/health` reports the running build's commit. To build on a server instead of pulling:
`docker compose -f compose.yaml -f compose.build.yml up -d --build`.

## How it fits together

```
Claude ──HTTPS──► Caddy ──► mpri-mcp (one process)
                              ├─ /mcp                 MCP tools (OAuth bearer tokens)
                              ├─ /authorize /token …  OAuth 2.1 authorization server
                              ├─ /admin, /oauth/…     dashboard, login, consent screen
                              ├─ scheduler            crawl:<supermarket> jobs, daily backup
                              ├─ crawler              Playwright → page digest → AI provider
                              └─ SQLite               ./data (Docker volume)
```

When Claude connects:
1. It registers itself as an OAuth client.
2. It sends you to this server's sign-in page. You sign in as the admin and approve.
3. Claude receives short-lived access tokens, refreshed automatically.
4. Under **Connected apps** you can revoke access at any time.

## MCP tools

| Tool | What it does |
|---|---|
| `get_supermarkets` | Supermarkets with schedule status, next run, last crawl result |
| `add_supermarket(name, base_url)` | Add a supermarket and return its ID |
| `setup_supermarket_cron_task(supermarket_id, cron_expression)` | Create or replace that supermarket's crawl schedule, effective immediately |
| `disable_supermarket_cron_task(supermarket_id)` | Pause a schedule (the expression is kept) |
| `trigger_crawl_now(supermarket_id)` | Crawl now in the background |
| `add_product_with_urls(name, urls[], category?)` | Track a product at one or more supermarkets |
| `get_product_price_comparison(product_name)` | Latest price everywhere, cheapest first. Fuzzy, accent-insensitive matching |
| `get_deep_discount_items(min_discount_percentage, days=7, limit=50)` | Products discounted at least N% |
| `get_price_history(product_name, days=30, supermarket_id?)` | Daily history with min, max, average and today's price vs. the period's low |

Cron expressions are standard 5-field cron in `Asia/Ho_Chi_Minh` time. Weekday 0/7 is Sunday,
which is converted correctly for APScheduler 3.

## Local development

Requires Python 3.12+ and [uv](https://docs.astral.sh/uv/).

```bash
uv sync
uv run playwright install --only-shell chromium   # add --with-deps on Linux
cp .env.example .env                              # set an AI provider (see below)
uv run mpri-mcp serve                             # http://localhost:8000/admin
```

On first start the log prints a **setup token**. Open `http://localhost:8000/admin` to
create the admin account. Run `uv run pytest` for the test suite. It needs no network or
API keys: AI providers and the browser are replaced with test doubles.

### Command line

| Command | Purpose |
|---|---|
| `mpri-mcp serve` | Run the server (the default command) |
| `mpri-mcp digest <url> [--supermarket ID]` | Fetch a page and print exactly what would be sent to the AI. No AI call |
| `mpri-mcp ai-test <url> [--provider P] [--model M]` | Fetch and extract one page without saving |
| `mpri-mcp crawl <supermarket-id> [--dry-run]` | Crawl one supermarket now |
| `mpri-mcp admin reset-password [--username NAME]` | Set a new admin password and sign out all sessions |
| `python scripts/smoke_test.py <base-url> <admin>` | End-to-end check of a running server (OAuth + MCP) |

## Configuration

All settings are environment variables (or `.env`). See [.env.example](.env.example) for the
full list.

| Variable | Default | Notes |
|---|---|---|
| `PUBLIC_BASE_URL` | `http://localhost:8000` | Public origin. The connector URL is `${PUBLIC_BASE_URL}/mcp` |
| `AI_PROVIDER` / `AI_MODEL` | `gemini` / `gemini-2.5-flash-lite` | Default extraction model. The dashboard and each supermarket can override it |
| `AI_FALLBACK_PROVIDER` / `AI_FALLBACK_MODEL` | — | Used when the main provider keeps failing |
| `GEMINI_BACKEND` | `vertex` | `vertex`: Google Cloud project + service account. `studio`: `GEMINI_API_KEY` |
| `GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_LOCATION` | — / `global` | Vertex AI |
| `OPENAI_API_KEY` / `OPENAI_BASE_URL` | — | OpenAI, or any compatible API (OpenRouter, DeepSeek, Groq, Ollama, vLLM…) |
| `ANTHROPIC_API_KEY` / `ANTHROPIC_EFFORT` | — / `low` | Claude models, e.g. `claude-opus-5-5`, or `claude-haiku-5-5` for lowest cost |
| `SCHEDULER_TIMEZONE` | `Asia/Ho_Chi_Minh` | Cron and display timezone |
| `MAX_CONCURRENT_MARKETS` / `MAX_PAGES_PER_MARKET` | `2` / `3` | Crawl concurrency |
| `PAGE_TIMEOUT_SECONDS` / `JOB_TIMEOUT_SECONDS` | `45` / `1800` | Per-page and per-crawl limits |
| `ADMIN_SETUP_TOKEN` | random, logged | First-run setup token |

API keys are read only from the environment. They are never stored in the database or
shown in the dashboard.

### Per-supermarket crawl settings

Each supermarket has an optional JSON `crawl_config`, edited on its dashboard page. It covers
sites that need a store to be chosen or a proxy:

```json
{
  "wait_for_selector": ".price",
  "text_selector": "main",
  "cookies": [{"name": "store_id", "value": "10010"}],
  "setup_actions": [{"action": "click", "selector": "#choose-store"}],
  "proxy": {"server": "http://vn-proxy.example.com:3128"}
}
```

## Security notes

- **OAuth 2.1:**
  - Authorization code flow with PKCE (S256) only.
  - Dynamic client registration accepts only Claude's callback and loopback redirect URIs.
  - Codes are single-use; reusing a code revokes the tokens it issued.
  - Refresh tokens rotate on every use; reusing an old one revokes that token family.
  - Tokens are stored as SHA-256 hashes.
- **First-run setup** needs a one-time token from the server logs, so nobody else can claim a fresh deployment.
- **Admin login:**
  - Passwords are hashed with argon2id.
  - The session cookie is signed, HttpOnly, SameSite=Lax, and Secure over HTTPS.
  - Changing the password signs out all sessions.
- **Forms and browser protections:**
  - CSRF tokens on every form.
  - Login rate limiting.
  - CSP `'self'` only, with no third-party scripts at runtime.
  - `X-Frame-Options: DENY`, so the consent screen can't be framed.

## Project layout

```
src/mpri_mcp/
  app.py            ASGI app: dashboard, OAuth server, MCP mount, middleware
  mcp_server.py     MCPServer (resource server) + tool registration
  container.py      wires db, AI registry, browser, runner, scheduler, OAuth provider
  tools/            MCP tools
  services/         business logic shared by tools and dashboard
  scheduler/        APScheduler engine, cron translation, SQLite backups
  crawler/          Playwright browser, page digest, AI extractor, crawl runner
  ai/               provider adapters (gemini, openai_compat, anthropic) + registry
  auth/             admin account, OAuth provider/routes, passwords
  admin/            dashboard routes, Jinja2 templates, static assets (vendored)
  db/               SQLAlchemy models + async session
tests/              pytest suite (fixtures include two real MM Mega Market pages)
deploy/             Caddyfile, VPS guide
compose.yaml        production: pulls MPRI_IMAGE from ghcr.io, plus Caddy
compose.build.yml   override to build the image locally instead
../.github/workflows/docker-publish.yml   CI: test, build, publish the image
```
