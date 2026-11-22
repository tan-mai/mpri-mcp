<!-- TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host -->
# Running MPriMCP on a VPS

`serve` mode behind Caddy (automatic HTTPS), with server-side cron, using the Docker image that
GitHub Actions publishes (`.github/workflows/docker-publish.yml`). No domain, token, IP or data
belongs in the repository: everything instance-specific lives in the VPS's `.env`.

**You need:** an x86_64 Linux VPS (512 MB RAM is plenty), Docker with the compose plugin, and a
domain whose DNS A record points at the VPS (Claude connects over HTTPS only).

## 1. Files on the server

```
~/mprimcp/
├── docker-compose.vps.yml   (deploy/docker-compose.vps.yml)
├── Caddyfile                (deploy/Caddyfile)
└── .env
```

`.env`:

```dotenv
MPRIMCP_IMAGE=ghcr.io/<owner>/mpri-mcp:sha-xxxxxxx   # from the Actions run summary
DOMAIN=mpri.example.com
# First start only (ignored once an account exists). At least 12 characters.
MPRIMCP_ADMIN_USER=admin
MPRIMCP_ADMIN_PASSWORD=change-me-please
```

The compose file sets `MPRIMCP_PUBLIC_URL=https://${DOMAIN}`, `MPRIMCP_CRON_ENABLED=true` and
`MPRIMCP_TRUST_PROXY=true`.

## 2. Start

```bash
cd ~/mprimcp
docker compose -f docker-compose.vps.yml pull
docker compose -f docker-compose.vps.yml up -d
curl -s https://mpri.example.com/health     # {"status":"ok","version":"…"}
```

Without `MPRIMCP_ADMIN_USER`/`MPRIMCP_ADMIN_PASSWORD` and without an existing account the container
refuses to start and says so in `docker compose logs mprimcp`. You can instead create the account
interactively: `docker compose -f docker-compose.vps.yml run --rm -it mprimcp user create`.

## 3. Connect an AI client

- claude.ai → Settings → Connectors → Add custom connector → `https://mpri.example.com/mcp`.
  Your browser opens this server's sign-in page; sign in and click **Cho phép**.
- Claude Code: `claude mcp add --transport http mprimcp https://mpri.example.com/mcp`.

## 4. Moving from the Python release (keeps Claude's connector working)

The Rust release keeps the same OAuth endpoints, token format and scope, so after the data is
imported Claude's existing connector keeps working: **same URL, no reconnect**. Two conditions:

1. `DOMAIN` must give the same public URL as the old `PUBLIC_BASE_URL` (tokens are bound to
   `https://<domain>/mcp`).
2. The data must be imported: supermarkets, products, URLs, price history, tasks, alerts,
   notification settings + Telegram/Discord secrets, the account (its argon2id hash is kept,
   so the password stays the same) and the OAuth clients/tokens.

**Option A — automatic, no shell commands needed:** mount the old data volume read-only and set
`MPRIMCP_IMPORT_FROM`. On the first start, while the new database is still empty, the server imports
`mpri.db` and the `secrets/` folder next to it, and logs the number of rows per table. On later
starts the variable is ignored. Add to `docker-compose.vps.yml`:

```yaml
services:
  mprimcp:
    environment:
      MPRIMCP_IMPORT_FROM: /legacy/mpri.db
    volumes:
      - legacy_data:/legacy:ro
volumes:
  legacy_data:
    external: true
    name: mpri_mpri_data        # the old volume: `docker volume ls | grep mpri_data`
```

Stop the old stack first (`docker compose down` in the old folder, volumes are kept), then start
the new one. The old volume is never written to, so rolling back is: stop the new stack, start the
old one.

**Option B — manual:** copy `mpri.db` (and `secrets/`) out of the old volume, then
`docker compose -f docker-compose.vps.yml run --rm -v /path/to/old:/legacy:ro mprimcp import --from /legacy/mpri.db`.
Running it twice inserts nothing new; the output lists `source / inserted / skipped` per table.

## Operating it

| Task | How |
|---|---|
| Deploy a new build | Put the new `sha-…` tag in `MPRIMCP_IMAGE`, then `pull` + `up -d` |
| Roll back | Put the previous tag back, then `pull` + `up -d` |
| Logs | `docker compose -f docker-compose.vps.yml logs -f mprimcp` |
| Forgot the password | `docker compose -f docker-compose.vps.yml exec -it mprimcp mprimcp user set-password <username>` |
| Backups | Daily at 04:30 Vietnam time, 7 kept, in `/data/backups` of the `mprimcp_data` volume |

Run exactly one `serve` container per database: the cron scheduler runs inside it.
