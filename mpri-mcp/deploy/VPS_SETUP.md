# Deploying mpri-mcp on a VPS

This guide takes a fresh Ubuntu VPS to a working Claude connector.

- **GitHub Actions** builds the Docker image on every push to `main` and publishes it to
  GitHub Container Registry (`ghcr.io/tan-mai/mpri-mcp`).
- **The VPS** pulls the exact build you choose and runs it with Docker Compose, behind Caddy,
  which handles HTTPS automatically.

**You need:**
- An Ubuntu 22.04/24.04 VPS on **x86_64** (check with `uname -m`) with **2 GB RAM or more**
  (Chromium), 1–2 vCPU, and about 5 GB of free disk.
- A domain or subdomain whose DNS **A record** points at the VPS, e.g. `mpri.example.com`.
  Claude only connects over HTTPS, and Let's Encrypt only issues certificates for names, not IP addresses.
  - *No domain yet?* A free wildcard-DNS name like `203-0-113-10.sslip.io` (your IP with dashes) usually works for testing.
    Move to your own domain later (see [Changing the domain](#changing-the-domain)).
- A Google Cloud project for Gemini on Vertex AI, or an API key for another provider.

---

## 1. Publish the image (once)

1. Push the code: `git push -u origin main`.
2. On GitHub, open **Actions → Build and publish image** and wait for the run to go green. It
   runs the tests, then builds and pushes the image. The first build takes a few minutes
   because it installs Chromium; later builds reuse the cache.
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
├── .env                 (from .env.example)
└── secrets/gcp-sa.json  (step 4)
```

**If the GitHub repository is public**, download them on the VPS:

```bash
mkdir -p ~/mpri/deploy ~/mpri/secrets && cd ~/mpri
base=https://raw.githubusercontent.com/tan-mai/mpri-mcp/main/mpri-mcp
curl -fsSLo compose.yaml      $base/compose.yaml
curl -fsSLo deploy/Caddyfile  $base/deploy/Caddyfile
curl -fsSLo .env              $base/.env.example
```

**If the repository is private**, first create the folders on the VPS
(`mkdir -p ~/mpri/deploy ~/mpri/secrets`). Then copy the files from your computer
(PowerShell, in `C:\Working\AI\MPRIMCP\mpri-mcp`):

```powershell
scp compose.yaml you@your-vps:~/mpri/
scp deploy/Caddyfile you@your-vps:~/mpri/deploy/
scp .env.example you@your-vps:~/mpri/.env
```

## 4. Gemini on Vertex AI (uses the $300 trial credit)

In the [Google Cloud console](https://console.cloud.google.com/):

1. Create (or pick) a project. Note its **project ID**.
2. **Billing** → link the project to your billing account. The free-trial credit lives there.
3. **APIs & Services → Enable APIs** → enable **Vertex AI API**.
4. **IAM & Admin → Service Accounts → Create service account**, e.g. `mpri-mcp`.
   Grant it the role **Vertex AI User**.
5. Open the service account → **Keys → Add key → JSON**. A file downloads.

Copy the key to the server and make it readable by the container user:

```bash
# from your computer
scp your-key.json you@your-vps:~/mpri/secrets/gcp-sa.json
# on the VPS
chmod 644 ~/mpri/secrets/gcp-sa.json
```

> Using another provider instead? Set `AI_PROVIDER` / `AI_MODEL` and that provider's key in
> `.env` (`OPENAI_API_KEY` + optional `OPENAI_BASE_URL`, or `ANTHROPIC_API_KEY`). You can
> switch providers later from the dashboard.

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
| `GOOGLE_CLOUD_PROJECT` | your project ID |
| `GOOGLE_CLOUD_LOCATION` | `global` (the default) |
| `AI_MODEL` | `gemini-2.5-flash-lite` (the default) |

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

**Can the VPS reach the supermarket sites?** Some Vietnamese sites block or geo-price
foreign datacenter IPs, and this VPS is in Malaysia. Run:

```bash
docker compose exec mpri-mcp mpri-mcp digest \
  https://online.mmvietnam.com/product/nuoc-tinh-khiet-satori-thung-24-chai-350ml.html
```

You should see the product title and prices such as `65,000 ₫` in the output. If you get
an error page or a captcha instead, give that supermarket a proxy in its **Crawl settings**
on the dashboard.

**Does AI extraction work?** Go to Dashboard → **AI** → **Test extraction**, paste the same
URL and run it. A Vertex error here usually means the API isn't enabled, billing isn't
linked, or the key file is missing or unreadable.

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

Claude Code can use the same server: `claude mcp add --transport http mpri https://mpri.example.com/mcp`.

---

## Operating it

| Task | Command / place |
|---|---|
| Deploy a new build | Copy the `MPRI_IMAGE=…:sha-…` line from the new Actions run summary into `~/mpri/.env`, then `docker compose pull && docker compose up -d`. Data and schedules are kept. |
| Roll back | Put the previous `sha-…` tag back in `MPRI_IMAGE`, then `docker compose pull && docker compose up -d` |
| Which build is running | `curl -s https://<domain>/health` (shows `commit`), or `docker compose images` |
| Logs | `docker compose logs -f mpri-mcp` |
| Forgot the admin password | `docker compose exec -it mpri-mcp mpri-mcp admin reset-password` |
| Crawl one supermarket from the shell | `docker compose exec mpri-mcp mpri-mcp crawl <supermarket-id>` |
| Revoke Claude's access | Dashboard → **Connected apps** → Revoke |
| Backups | Written daily (keeps 7) to `/app/data/backups` in the `mpri_data` volume. Copy them off the server: `docker compose cp mpri-mcp:/app/data/backups ./backups` |
| Restore a backup | `docker compose stop mpri-mcp`, then `docker compose cp ./backups/mpri-XXXX.db mpri-mcp:/app/data/mpri.db`, then `docker compose start mpri-mcp` |
| Clean up old images | `docker image prune` (removes images no container uses) |

When a release changes `compose.yaml` or `deploy/Caddyfile`, download or copy those two files
again as in step 3. Most releases only change the image.

**Keep exactly one app container.** The scheduler runs inside it, so scaling the service to
more than one replica would crawl everything twice.

**Building on the server instead** (for example if GitHub Actions is unavailable): clone the
repository, `cd mpri-mcp`, copy your `.env` and `secrets/` there, and run
`docker compose -f compose.yaml -f compose.build.yml up -d --build`.

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
| Every crawl fails with `HTTP 403` or a captcha | The site blocks the VPS IP. Add a `proxy` in that supermarket's crawl settings. |
| Crawls fail with an AI provider error | Dashboard → AI shows each provider's status. Check its credentials in `.env`, then run `docker compose up -d`. |
| `/admin/setup` returns 404 | An admin already exists. Use `admin reset-password` (above). |
