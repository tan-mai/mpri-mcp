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
imported Claude's existing connector keeps working: **same URL, no reconnect**. The import copies
supermarkets, products, URLs, price history, tasks, alerts, notification settings + Telegram/Discord
secrets, the account (its argon2id hash is kept, so the password stays the same) and the OAuth
clients/tokens.

> **Before pushing the Rust code to `main`:** every push to `main` moves the `:latest` image tag.
> The old `compose.yaml` cannot run the Rust image (different variables and data path), so the
> VPS's `MPRI_IMAGE` must be pinned to a `sha-…` tag, not `:latest`, until the switch below.

Do the switch in the old folder (`~/mpri`), so the Compose project name stays `mpri` and Caddy keeps
its certificates (`mpri_caddy_data`):

1. Note the old data volume and stop the old stack (volumes are kept):

   ```bash
   cd ~/mpri
   docker volume ls | grep mpri_data       # usually mpri_mpri_data
   docker compose down
   ```

2. Copy `deploy/docker-compose.vps.yml` and `deploy/Caddyfile` from this release into `~/mpri/`
   (next to the old `compose.yaml`, which is no longer used). In `docker-compose.vps.yml` add the
   one-time import of the old volume (mounted read-only, never modified):

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
       name: mpri_mpri_data
   ```

3. Add to `~/mpri/.env` (the old variables can stay; they are ignored):

   ```dotenv
   MPRIMCP_IMAGE=ghcr.io/<owner>/mpri-mcp:sha-xxxxxxx   # the Rust build from the Actions summary
   DOMAIN=mpri.example.com                             # must give the same URL as the old PUBLIC_BASE_URL
   ```

   Do **not** set `MPRIMCP_ADMIN_USER`/`MPRIMCP_ADMIN_PASSWORD` for the switch: the account comes from
   the import. If the import cannot find the file, the server then refuses to start instead of
   running with an empty database.

4. Start and check:

   ```bash
   docker compose -f docker-compose.vps.yml pull
   docker compose -f docker-compose.vps.yml up -d
   docker compose -f docker-compose.vps.yml logs mprimcp | grep -E "imported|started|error"
   curl -s https://mpri.example.com/health        # "version":"0.2.0"
   ```

   The log lists the rows imported per table. Then sign in to `https://<domain>/admin` with the old
   password and ask Claude for `get_supermarkets`: it must answer without asking to reconnect.

5. Later starts skip the import (the database is no longer empty). You may remove the
   `MPRIMCP_IMPORT_FROM` lines after the first successful start.

**Roll back:** `docker compose -f docker-compose.vps.yml down`, then `docker compose up -d` (the old
`compose.yaml` with its pinned `MPRI_IMAGE`). The old volume was only read, so the Python release
resumes exactly where it stopped.

**Manual import instead:** with the old stack stopped,
`docker compose -f docker-compose.vps.yml run --rm -v mpri_mpri_data:/legacy:ro mprimcp import --from /legacy/mpri.db`.
Running it twice inserts nothing new; the output lists `source / inserted / skipped` per table.

## Language after upgrading

Releases with English/Vietnamese support start in English. To keep the dashboard and the
Telegram/Discord messages in Vietnamese, set `MPRIMCP_LANGUAGE: vi` in the service's environment, or
pick *Tiếng Việt* on the dashboard's account page.

## Operating it

| Task | How |
|---|---|
| Deploy a new build | Put the new `sha-…` tag in `MPRIMCP_IMAGE`, then `pull` + `up -d` |
| Roll back | Put the previous tag back, then `pull` + `up -d` |
| Logs | `docker compose -f docker-compose.vps.yml logs -f mprimcp` |
| Forgot the password | `docker compose -f docker-compose.vps.yml exec -it mprimcp mprimcp user set-password <username>` |
| Backups | Daily at 04:30 Vietnam time, 7 kept, in `/data/backups` of the `mprimcp_data` volume |

Run exactly one `serve` container per database: the cron scheduler runs inside it.
