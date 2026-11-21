# Deploying mpri-mcp on a VPS

This guide takes a fresh Ubuntu VPS to a working Claude connector.

- **GitHub Actions** builds the Docker image on every push to `main` and publishes it to
  GitHub Container Registry (`ghcr.io/tan-mai/mpri-mcp`).
- **The VPS** pulls the exact build you choose and runs it with Docker Compose, behind Caddy,
  which handles HTTPS automatically.

**You need:**
<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->
- An Ubuntu 22.04/24.04 VPS on **x86_64** (check with `uname -m`) with **1 GB RAM or more**,
  1 vCPU, and about 2 GB of free disk. The server crawls nothing itself: Claude reads the
  prices with its browser, so no AI key and no Chromium are needed.
- A domain or subdomain whose DNS **A record** points at the VPS, e.g. `mpri.example.com`.
  Claude only connects over HTTPS, and Let's Encrypt only issues certificates for names, not IP addresses.
  - *No domain yet?* A free wildcard-DNS name like `203-0-113-10.sslip.io` (your IP with dashes) usually works for testing.
    Move to your own domain later (see [Changing the domain](#changing-the-domain)).

---

## 1. Publish the image (once)

1. Push the code: `git push -u origin main`.
2. On GitHub, open **Actions → Build and publish image** and wait for the run to go green. It
   runs the tests, then builds and pushes the image.
3. The run's **Summary** shows the image to deploy, for example:
   ```
   MPRI_IMAGE=ghcr.io/tan-mai/mpri-mcp:sha-3ef0a94
   ```
4. **Make the package public (one time only).** New packages start private. On GitHub, open
   your profile → **Packages** → **mpri-mcp** → **Package settings** → *Danger Zone* →
   **Change visibility** → **Public**, then type the package name to confirm.
   This cannot be undone. The image contains the app code but no secrets.

From now on, every push to `main` that changes the app publishes a new `sha-…` tag and moves
`latest`. Pushing a git tag such as `v1.0.0` also publishes `:1.0.0`.

## 2. Prepare the server

```bash
# Docker Engine + the compose plugin
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker $USER   # log out and back in afterwards

# Firewall: SSH + HTTP/HTTPS only
sudo ufw allow OpenSSH
sudo ufw allow 80/tcp
sudo ufw allow 443
sudo ufw enable

# Check the image is reachable (no login needed once it's public)
docker pull ghcr.io/tan-mai/mpri-mcp:latest
```

## 3. Get the deployment files

The VPS needs only three files, not the source code:

```
~/mpri/
├── compose.yaml
├── deploy/Caddyfile
└── .env                 (from .env.example)
```

**If the GitHub repository is public**, download them on the VPS:

```bash
mkdir -p ~/mpri/deploy && cd ~/mpri
base=https://raw.githubusercontent.com/tan-mai/mpri-mcp/main/mpri-mcp
curl -fsSLo compose.yaml      $base/compose.yaml
curl -fsSLo deploy/Caddyfile  $base/deploy/Caddyfile
curl -fsSLo .env              $base/.env.example
```

**If the repository is private**, first create the folders on the VPS
(`mkdir -p ~/mpri/deploy`). Then copy the files from your computer
(PowerShell, in `C:\Working\AI\MPRIMCP\mpri-mcp`):

```powershell
scp compose.yaml you@your-vps:~/mpri/
scp deploy/Caddyfile you@your-vps:~/mpri/deploy/
scp .env.example you@your-vps:~/mpri/.env
```

## 4. (no AI provider needed)

<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->
Earlier releases read prices with Gemini on Vertex AI. This release has no server crawler,
so there is no AI key or service-account file to set up.

## 5. Configure

```bash
cd ~/mpri
nano .env
```

Set at least these values:

| Variable | Value |
|---|---|
| `MPRI_IMAGE` | the image from the Actions run summary, e.g. `ghcr.io/tan-mai/mpri-mcp:sha-3ef0a94` |
| `DOMAIN` | `mpri.example.com` |
| `PUBLIC_BASE_URL` | `https://mpri.example.com`: exactly what you will give Claude, without `/mcp` |

Pinning `MPRI_IMAGE` to a `sha-…` tag means the server only changes when you change it.
Optionally set `ADMIN_SETUP_TOKEN` to a value you choose. Otherwise a random token is
printed in the logs on first start.

## 6. Start

```bash
cd ~/mpri
docker compose pull
docker compose up -d
docker compose ps                         # both services "running"; mpri-mcp becomes "healthy"
docker compose logs mpri-mcp | grep -A3 "FIRST-RUN SETUP"
curl -s https://mpri.example.com/health   # {"status":"ok","version":"0.1.0","commit":"3ef0a94"}
```

The log shows the **setup token**. The `commit` in `/health` tells you which build is running.

## 7. Create the admin account

Open `https://mpri.example.com/admin`. You'll be sent to **First-run setup**. Enter the setup
token, a username and a password of at least 12 characters. This account signs in to the
dashboard and approves Claude's connection.

## 8. Check it works (recommended)

**Does OAuth + MCP work end to end?** From your computer, in the repo:

```bash
uv run python scripts/smoke_test.py https://mpri.example.com <admin-username>
```

## 9. Connect Claude

1. In claude.ai open **Settings → Connectors → Add custom connector**.
2. Enter name **MPRI** and URL **`https://mpri.example.com/mcp`**.
3. Click **Connect**. Your browser opens this server's sign-in page. Sign in as the admin and click **Allow**.

Then try:

> Hãy thêm mới siêu thị MM Mega Market với domain online.mmvietnam.com.
>
> Thiết lập lịch quét dữ liệu cho MM Mega Market vào lúc 3h sáng mỗi ngày giúp tôi.
>
> Tôi muốn theo dõi sản phẩm "Nước tinh khiết Satori, thùng 24 chai, 350ml", link: https://online.mmvietnam.com/product/nuoc-tinh-khiet-satori-thung-24-chai-350ml.html
>
> Quét giá ngay bây giờ giúp tôi, rồi so sánh giá sản phẩm Satori.

<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->
"Quét giá" queues crawl tasks; Claude then opens each product page with Claude in Chrome /
Claude Browser (the computer must be on with Chrome open) and sends the prices back. To run it
on a schedule, create a scheduled task in Claude after the supermarkets' cron times, e.g. 11:30
on Tuesday and Saturday, with the prompt *"Run the MPriMCP agent crawl workflow until no pending
tasks remain."*

Claude Code can use the same server: `claude mcp add --transport http mpri https://mpri.example.com/mcp`.

<!-- TM - #{ticket number} - MPriMCP Price Alerts & Notifications -->
## 10. Notifications (optional)

Dashboard → **Notifications**: paste a Telegram bot token (from @BotFather) and detect the
chat ID, and/or a Discord webhook URL, switch the channel on and press **Send test**. No
server access is needed: the token and URL are stored in the data volume.

---

## Operating it

| Task | Command / place |
|---|---|
| Deploy a new build | Copy the `MPRI_IMAGE=…:sha-…` line from the new Actions run summary into `~/mpri/.env`, then `docker compose pull && docker compose up -d`. Data and schedules are kept. |
| Roll back | Put the previous `sha-…` tag back in `MPRI_IMAGE`, then `docker compose pull && docker compose up -d` |
| Which build is running | `curl -s https://<domain>/health` (shows `commit`), or `docker compose images` |
| Logs | `docker compose logs -f mpri-mcp` |
| Forgot the admin password | `docker compose exec -it mpri-mcp mpri-mcp admin reset-password` |
| Revoke Claude's access | Dashboard → **Connected apps** → Revoke |
| Backups | Written daily (keeps 7) to `/app/data/backups` in the `mpri_data` volume. Copy them off the server: `docker compose cp mpri-mcp:/app/data/backups ./backups` |
| Restore a backup | `docker compose stop mpri-mcp`, then `docker compose cp ./backups/mpri-XXXX.db mpri-mcp:/app/data/mpri.db`, then `docker compose start mpri-mcp` |
| Clean up old images | `docker image prune` (removes images no container uses) |

When a release changes `compose.yaml` or `deploy/Caddyfile`, download or copy those two files
again as in step 3. Most releases only change the image.

**Keep exactly one app container.** The scheduler runs inside it, so scaling the service to
more than one replica would queue every crawl twice.

**Building on the server instead** (for example if GitHub Actions is unavailable): clone the
repository, `cd mpri-mcp`, copy your `.env` there, and run
`docker compose -f compose.yaml -f compose.build.yml up -d --build`.

<!-- TM - #{ticket number} - MPriMCP Agent Crawl & Product Search -->
### Upgrading to the agent-crawl release (data reset, automatic)

Deploy the new image as usual; no commands are needed. On its first start the server:

1. finds the database of the server-crawler release,
2. copies it to `/app/data/backups/legacy-before-agent-crawl-<time>.db` (kept, not rotated),
3. removes supermarkets, products, prices and crawl history, and creates the new tables.

The admin account and Claude's connector are kept: sign in to the dashboard with the same
password, and Claude keeps working without reconnecting. Then set the supermarkets (with
their search URL templates), schedules and products up again through Claude. The old
`AI_*`, `GEMINI_*`, `GOOGLE_*` lines in `.env` and the `secrets/` folder are ignored and can
be deleted whenever convenient.

### Changing the domain

1. Point the new DNS name at the VPS.
2. In `.env`, set both `DOMAIN` and `PUBLIC_BASE_URL` to the new name.
3. Run `docker compose up -d`.
4. In claude.ai, remove the connector and add it again with the new URL. Tokens are bound to the old URL, so you'll sign in again.

### Troubleshooting

| Symptom | Likely cause |
|---|---|
| `docker compose pull` says `denied` or `unauthorized` | The package is still private: make it public (step 1.4). Also check the tag exists in the Actions summary. |
| `no matching manifest for linux/arm64` | The VPS is ARM (`uname -m` = `aarch64`). Add `linux/arm64` to `platforms` in `.github/workflows/docker-publish.yml`, push, and use the new tag. |
| The Actions run fails at "Lint and test" | A test or lint error. The image is only published when tests pass, so the VPS keeps running the previous build. |
| Claude says "Couldn't reach the MCP server" | `PUBLIC_BASE_URL` doesn't exactly match the URL you entered (scheme, host, no trailing slash), or DNS/HTTPS isn't working yet. Open `https://<domain>/.well-known/oauth-protected-resource/mcp` in a browser: its `resource` must equal your connector URL. |
| `421 Invalid Host header` | The request's host isn't the host in `PUBLIC_BASE_URL`. |
| Caddy logs certificate errors | The DNS A record isn't pointing at this server yet, or port 80/443 is blocked. |
| `mpri-mcp` keeps restarting; the log says the database "does not match this version" | The data volume holds a database the release does not recognise. Restore a backup made by the same release, or start with an empty volume. |
| Prices never update | Crawl tasks only queue up (`pending_tasks` in `get_supermarkets`). Ask Claude to run the agent crawl, or check its scheduled task. |
| `/admin/setup` returns 404 | An admin already exists. Use `admin reset-password` (above). |
