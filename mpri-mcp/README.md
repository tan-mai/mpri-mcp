# mpri-mcp — Market Pricing MCP server

Tracks product prices at Vietnamese supermarkets (MM Mega Market, Winmart, Co.op, Go!, …)
and gives Claude tools to manage the tracking and analyse prices, as a **remote MCP
connector** secured with OAuth 2.1.

<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->
<!-- TM - #{ticket number} - MPriMCP Price Alerts & Notifications -->
- **16 MCP tools.** Add supermarkets, set each one's independent crawl schedule, track products by URL, queue crawls, compare prices, find deep discounts, read price history, build search URLs, run the agent crawl, and set price alerts.
- **Telegram / Discord notifications.** A message as soon as a crawl saves a price below one of your price alerts, and after every finished agent crawl a summary of where each tracked product is cheapest. Channels are set up on the dashboard (bot token and webhook URL are kept out of the database).
- **Agent crawl, no AI key.** The server never fetches a web page. Each supermarket's cron job (or `trigger_crawl_now`) queues crawl tasks. Claude takes them with `get_pending_crawl_tasks`, reads each product page with Claude in Chrome / Claude Browser, and sends `price`, `original_price` and `member_price` back with `submit_price_results`.
- **Per-supermarket cron jobs.** One schedule per market (APScheduler, standard cron, Vietnam time).
- **Product search by name.** `get_search_urls` builds each supermarket's search-results URL from its `search_url_template`; Claude opens them, groups results by identical spec and saves only what you confirm.
- **Price checks.** Invalid results are rejected per item; a price more than 50% away from the previous one is saved with `needs_review` and left out of deep-discount results until you accept it in the dashboard.
- **Admin dashboard.** The admin account is created on the first visit. Pages: overview (with prices to review), supermarkets, products with price charts and price alerts, crawl runs, notifications, and connected apps. The MPRI avatar is the dashboard logo, the favicon, and the connector icon (`serverInfo.icons`).
- **Self-contained.** One process and one SQLite file, no browser in the image. GitHub Actions publishes the Docker image to `ghcr.io/tan-mai/mpri-mcp`; the VPS runs it with Docker Compose and Caddy (automatic HTTPS).

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
                              ├─ scheduler            crawl:<supermarket> jobs queue crawl tasks
                              └─ SQLite               ./data (Docker volume)

Claude (browser) ◄── get_pending_crawl_tasks ── crawl_tasks (pending → claimed → done/failed)
Claude (browser) ──► submit_price_results ──► price_records (+ needs_review) → crawl run result
```
<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->

A claimed task is reserved for 30 minutes (`AGENT_TASK_LEASE_MINUTES`), then returns to the
queue. A result with status `error` re-queues the task until it has been claimed 3 times
(`AGENT_TASK_MAX_ATTEMPTS`). Tasks still pending after 48 hours (`AGENT_TASK_EXPIRE_HOURS`)
are marked expired. When every task of a run has ended, the run gets `trigger = agent` and its
ok / failed counts, which `get_supermarkets` shows as `last_run`.

When Claude connects:
1. It registers itself as an OAuth client.
2. It sends you to this server's sign-in page. You sign in as the admin and approve.
3. Claude receives short-lived access tokens, refreshed automatically.
4. Under **Connected apps** you can revoke access at any time.

## MCP tools

| Tool | What it does |
|---|---|
| `get_supermarkets` | Supermarkets with schedule status, next run, last crawl result, `search_url_template`, `pending_tasks`, `needs_review_count` |
| `add_supermarket(name, base_url, search_url_template?)` | Add a supermarket and return its ID |
| `update_supermarket(supermarket_id, name?, base_url?, search_url_template?)` | Change only the fields passed (`""` removes the template) |
| `setup_supermarket_cron_task(supermarket_id, cron_expression)` | Create or replace that supermarket's crawl schedule, effective immediately |
| `disable_supermarket_cron_task(supermarket_id)` | Pause a schedule (the expression is kept) |
| `trigger_crawl_now(supermarket_id)` | Queue crawl tasks for its tracked pages now (no duplicates) |
| `get_pending_crawl_tasks(supermarket_id?, limit=20)` | Claim the oldest pending tasks (lease) for Claude to read in the browser |
| `submit_price_results(results[])` | Send the prices read (`ok` / `out_of_stock` / `not_found` / `error`), up to 50 per call |
| `get_search_urls(keyword, supermarket_ids?)` | Search-results URL per supermarket (keyword URL-encoded); nothing is fetched |
| `set_price_alert(product_name, target_price, supermarket_ids?)` | Message the user when a saved price is below the target (one alert per product) |
| `get_price_alerts()` | Alerts with their target and the cheapest current price |
| `delete_price_alert(product_name)` | Remove a product's alert |
| `add_product_with_urls(name, urls[], category?)` | Track a product at one or more supermarkets |
| `get_product_price_comparison(product_name)` | Latest price everywhere, cheapest first, with `source` and `needs_review`. Fuzzy, accent-insensitive matching |
| `get_deep_discount_items(min_discount_percentage, days=7, limit=50)` | Products discounted at least N% (prices flagged `needs_review` are left out) |
| `get_price_history(product_name, days=30, supermarket_id?)` | Daily history with min, max, average and today's price vs. the period's low |

<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->
The MCP server instructions tell Claude the two workflows: **agent crawl** (take tasks, read
prices, submit until `remaining_pending` is 0) and **add a product by search** (search every
supermarket, group by identical spec, wait for your confirmation, then `add_product_with_urls`).

Cron expressions are standard 5-field cron in `Asia/Ho_Chi_Minh` time. Weekday 0/7 is Sunday,
which is converted correctly for APScheduler 3.

## Local development

Requires Python 3.12+ and [uv](https://docs.astral.sh/uv/).

<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->
```bash
uv sync
cp .env.example .env          # no API keys needed
uv run mpri-mcp serve         # http://localhost:8000/admin
```

On first start the log prints a **setup token**. Open `http://localhost:8000/admin` to
create the admin account. Run `uv run pytest` for the test suite. It needs no network.

### Command line

| Command | Purpose |
|---|---|
| `mpri-mcp serve` | Run the server (the default command) |
| `mpri-mcp admin reset-password [--username NAME]` | Set a new admin password and sign out all sessions |
| `python scripts/smoke_test.py <base-url> <admin>` | End-to-end check of a running server (OAuth + MCP) |

## Configuration

All settings are environment variables (or `.env`). See [.env.example](.env.example) for the
full list.

| Variable | Default | Notes |
|---|---|---|
| `PUBLIC_BASE_URL` | `http://localhost:8000` | Public origin. The connector URL is `${PUBLIC_BASE_URL}/mcp` |
| `SCHEDULER_TIMEZONE` | `Asia/Ho_Chi_Minh` | Cron and display timezone |
| `AGENT_TASK_LEASE_MINUTES` | `30` | How long a claimed crawl task stays reserved |
| `AGENT_TASK_EXPIRE_HOURS` | `48` | Pending tasks older than this are marked expired |
| `AGENT_TASK_MAX_ATTEMPTS` | `3` | Claims per task before an `error` result fails it |
| `TELEGRAM_BOT_TOKEN` / `DISCORD_WEBHOOK_URL` | — | Optional; normally entered on the dashboard (Notifications) |
| `ADMIN_SETUP_TOKEN` | random, logged | First-run setup token |

<!-- TM - #{ticket number} - MPriMCP Price Alerts & Notifications -->
### Notifications

Dashboard → **Notifications**:
- **Telegram**: create a bot with @BotFather, paste its token, send `/start` to the bot, press
  **Detect chat ID**, switch it on and press **Send test**.
- **Discord**: Channel settings → Integrations → Webhooks → copy the webhook URL, paste it,
  switch it on.

Every message goes to each channel that is on. A price alert fires once per supermarket when
the price drops below the target, again if it drops further, and re-arms once the price is
back at or above the target. The summary is sent when a `submit_price_results` call leaves no
pending or claimed task (one per crawl round) and can be switched off. Every message is
listed under **Recent messages**; failures are retried and never break the crawl.

### Search URL template

Give each supermarket a `search_url_template` (dashboard or `update_supermarket`): search on
the site, copy the results page URL and put `{q}` where the keyword is, e.g.
`https://cooponline.vn/search?router=productListing&query={q}`. It must be an http(s) URL with
exactly one `{q}`.

### Database

Deploying this release needs no commands. On its first start the server finds the database
of the server-crawler release, copies it to `backups/legacy-before-agent-crawl-<time>.db` in
the data volume, and resets it: supermarkets, products, prices and crawl history are removed,
while the admin account and Claude's connector (OAuth clients and tokens) are kept. This
happens once; later starts leave the data alone. Any other schema mismatch stops the server
with a clear log message instead of deleting anything.

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

<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->

```
src/mpri_mcp/
  app.py            ASGI app: dashboard, OAuth server, MCP mount, middleware
  mcp_server.py     MCPServer (resource server) + tool registration
  container.py      wires db, scheduler (queues crawl tasks), OAuth provider
  tools/            MCP tools (setup, analytics, agent crawl)
  services/         business logic shared by tools and dashboard (agent_tasks: crawl queue)
  scheduler/        APScheduler engine, cron translation, SQLite backups
  auth/             admin account, OAuth provider/routes, passwords
  admin/            dashboard routes, Jinja2 templates, static assets (vendored)
  db/               SQLAlchemy models + async session
tests/              pytest suite
deploy/             Caddyfile, VPS guide
compose.yaml        production: pulls MPRI_IMAGE from ghcr.io, plus Caddy
compose.build.yml   override to build the image locally instead
../.github/workflows/docker-publish.yml   CI: test, build, publish the image
```
