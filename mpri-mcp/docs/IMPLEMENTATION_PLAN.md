# Implementation Plan — Market Pricing MCP Server (v3, approved 2026-10-20)

This is the plan the project owner approved before coding started (see
[REQUIREMENTS.md](REQUIREMENTS.md) §1). It records the design and the decisions that came
out of the planning discussion.

## Confirmed decisions

| Topic | Decision |
|---|---|
| Hosting | Docker Compose on a 24/7 Linux VPS (located in Malaysia). Claude connects through a claude.ai **custom connector** over HTTPS. A domain is registered at deployment time. |
| Transport | One long-running process: Streamable HTTP MCP endpoint + scheduler in the same process. Exactly **one** uvicorn worker. |
| Auth | OAuth 2.1 from day one. The app embeds its own small Authorization Server; the MCP endpoint (MCP SDK v2 `MCPServer`) is a pure Resource Server that verifies tokens against SQLite. |
| Admin | One admin account, created on the first visit to `/admin` after providing a one-time setup token printed in the container logs. The same login approves Claude's OAuth consent. |
| Dashboard | Server-rendered (FastAPI + Jinja2 + HTMX + Pico.css + Chart.js, all vendored — no CDN at runtime). |
| Database | SQLite (WAL mode) in a Docker volume, nightly backup. |
| AI | Provider adapter: Gemini (Vertex AI or AI Studio), any OpenAI-compatible API, Anthropic. Default: Gemini on Vertex AI (paid from the $300 GCP trial credit), model `gemini-2.5-flash-lite`. Switchable per environment, from the dashboard, or per supermarket; optional fallback provider. |
| Repo | Code lives in the main repository under `mpri-mcp/`; published to `github.com/tan-mai/mpri-mcp`. |
| Extra MCP tools | `get_price_history`, `trigger_crawl_now`, `disable_supermarket_cron_task` (in addition to the six in the spec). |
| Test pages | `online.mmvietnam.com` (MM Mega Market): Satori water 24×500ml and 24×350ml. |

## Architecture

```
        Admin (browser)                       Claude (claude.ai / Desktop / mobile)
             │ HTTPS                                   │ HTTPS (Anthropic egress 160.79.104.0/21)
             ▼                                         ▼
┌─────────────────────────── VPS · Docker Compose ───────────────────────────┐
│ caddy :443 — Let's Encrypt TLS, forwards client IP                          │
│  └─► mpri-mcp :8000 — ONE process, ONE uvicorn worker (FastAPI parent app)  │
│       ├─ /admin/*            Admin dashboard (session cookie)               │
│       ├─ /oauth/consent      Login + "Allow Claude?" consent                │
│       ├─ /.well-known/oauth-authorization-server                            │
│       │  /authorize /token /register /revoke   OAuth 2.1 AS (own module)    │
│       ├─ /mcp + /.well-known/oauth-protected-resource/mcp                   │
│       │                      MCPServer (SDK v2) — Resource Server           │
│       ├─ /health · /static/*                                                │
│       ├─ Scheduler: crawl:<market> × N · backup                             │
│       ├─ Crawler: Chromium (Playwright) → AI adapter → Gemini/OpenAI/Claude │
│       └─ SQLite (volume)                                                    │
└─────────────────────────────────────────────────────────────────────────────┘
```

Invariants: one worker; schedules and auth state persist in SQLite so redeploys lose
nothing; AI credentials only ever come from `.env` (never stored in the DB or shown in
the UI).

## Authentication

### Admin account (first run)
1. No admin exists → the app logs a one-time **setup token**; every `/admin/*` page
   redirects to `/admin/setup`.
2. Setup form: token + username + password (≥ 12 chars) ×2 → admin created, logged in.
   `/admin/setup` then returns 404 forever.
3. Recovery: `mpri-mcp admin reset-password` inside the container.

### OAuth flow (connecting Claude)
1. claude.ai → add custom connector `https://<domain>/mcp`.
2. `/mcp` answers `401` + `WWW-Authenticate: Bearer resource_metadata=…`.
3. Claude reads protected-resource and AS metadata, registers via `/register` (DCR).
4. Browser → `/authorize` (client, redirect URI, PKCE S256, `resource` validated) →
   `/oauth/consent`.
5. Login if needed → consent shows client name + redirect host → **Allow**.
6. One-time code (5 min, includes `iss`) → redirect to `https://claude.ai/api/mcp/auth_callback`.
7. `/token` → access token (1 h) + refresh token (30 d, rotated each use; reuse revokes the family).
8. `/mcp` with Bearer token.
9. Dashboard → **Connected apps**: list + revoke.

Claude-specific rules honoured: 401 (never 200) to start auth; `resource` equals the
connector URL exactly; form-urlencoded `/token`; `invalid_grant` errors; port-agnostic
loopback redirect matching (Claude Code, MCP Inspector); DCR redirect URIs restricted
to an allow-list.

### Hardening
argon2id password hashes; tokens are 256-bit random values stored as SHA-256 hashes;
HttpOnly/Secure/SameSite=Lax session cookie with idle timeout and a per-user
`session_version` (password change = logout everywhere); CSRF tokens on all forms;
login rate limit (5 failures / 15 min per IP and per username); CSP `'self'` only,
`X-Frame-Options: DENY`; uvicorn trusts forwarded headers only from the proxy;
`MCPServer` `allowed_hosts` = the public host.

## AI provider adapter

```python
class LLMProvider(ABC):
    name: str
    def is_configured(self) -> bool: ...
    async def generate_structured(self, *, system, prompt, schema, model) -> StructuredResult[T]: ...
```

| Adapter | SDK | Covers |
|---|---|---|
| `gemini` (`backend=vertex\|studio`) | `google-genai` | Gemini on Vertex AI or AI Studio |
| `openai` | `openai` | OpenAI and any OpenAI-compatible `base_url` (OpenRouter, DeepSeek, Groq, Ollama, vLLM…) |
| `anthropic` | `anthropic` | Claude models |

Choice resolution: supermarket override → dashboard setting → `.env` default.
Retryable errors (429/5xx/timeouts) are retried with backoff, then the optional fallback
provider is tried. Every call records provider, model and token usage.

## Data model (12 tables)

| Group | Tables |
|---|---|
| Domain | `supermarkets` (cron, crawl_config, AI override), `products`, `product_urls`, `price_records` |
| Crawl | `crawl_runs`, `crawl_run_items` (per URL: status, error, duration, provider, model, tokens) |
| Auth | `admin_users`, `oauth_clients`, `oauth_pending_authorizations`, `oauth_authorization_codes`, `oauth_tokens` |
| Settings | `app_settings` (active AI provider/model, fallback, price table) |

## MCP tools

| Tool | Kind |
|---|---|
| `get_supermarkets` | read |
| `add_supermarket(name, base_url)` | write |
| `setup_supermarket_cron_task(supermarket_id, cron_expression)` | write |
| `disable_supermarket_cron_task(supermarket_id)` | write |
| `trigger_crawl_now(supermarket_id)` | write |
| `add_product_with_urls(name, category?, urls[])` | write |
| `get_product_price_comparison(product_name)` | read |
| `get_deep_discount_items(min_discount_percentage, days=7, limit=50)` | read |
| `get_price_history(product_name, days=30, supermarket_id?)` | read |

## Phases

| # | Phase | Done when |
|---|---|---|
| 0 | Repo prep | main repo tracks `mpri-mcp/` |
| 1 | Skeleton (uv, FastAPI app, config, logging, `/health`, Dockerfile) | app starts, `/health` = 200 |
| 2 | DB + services | unit tests green |
| 3 | Scheduler | add/update/pause/run-now/restart tests green |
| 4 | AI adapter layer | adapter contract tests green (mocked) |
| 5 | Crawler | dry-run on the MM Vietnam test URLs; also from the VPS (IP blocking check) |
| 6 | Admin auth | setup/login/lockout/session tests green |
| 7 | OAuth AS + MCP tools | full OAuth flow test; MCP Inspector; claude.ai via public URL |
| 8 | Admin dashboard | manual walkthrough desktop + phone width |
| 9 | Local end-to-end | spec §5 scenario passes |
| 10 | VPS deployment | first-run setup on VPS, claude.ai connector works |

## Risks

- VN supermarket sites may block or geo-price the Malaysian VPS IP → per-market
  `crawl_config` (cookies, setup actions, proxy); verify early from the VPS.
- Own OAuth server → covered by end-to-end tests of every rule.
- Model retirement / trial credit expiry → one setting change; fallback provider.
