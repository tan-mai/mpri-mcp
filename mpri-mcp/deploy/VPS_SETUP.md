# Deploying mpri-mcp on a VPS

This guide takes a fresh Linux VPS to a working Claude connector: Docker Compose runs the
app behind Caddy, which handles HTTPS automatically.

**You need:**
- A VPS with Ubuntu 22.04/24.04 (or Debian 12), **2 GB RAM or more** (Chromium), 1–2 vCPU, and about 5 GB of free disk.
- A domain or subdomain whose DNS **A record** points at the VPS, e.g. `mpri.example.com`.
  Claude only connects over HTTPS, and Let's Encrypt only issues certificates for names, not IP addresses.
  - *No domain yet?* A free wildcard-DNS name like `203-0-113-10.sslip.io` (your IP with dashes) usually works for testing.
    Move to your own domain later (see [Changing the domain](#changing-the-domain)).
- A Google Cloud project for Gemini on Vertex AI, or an API key for another provider.

---

## 1. Prepare the server

```bash
# Docker Engine + the compose plugin
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker $USER   # log out and back in afterwards

# Firewall: SSH + HTTP/HTTPS only
sudo ufw allow OpenSSH
sudo ufw allow 80/tcp
sudo ufw allow 443
sudo ufw enable
```

## 2. Get the code

```bash
git clone https://github.com/tan-mai/mpri-mcp.git
cd mpri-mcp/mpri-mcp          # the app lives in the mpri-mcp/ folder of the repo
mkdir -p secrets
```

## 3. Gemini on Vertex AI (uses the $300 trial credit)

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
scp ~/Downloads/your-key.json you@your-vps:~/mpri-mcp/mpri-mcp/secrets/gcp-sa.json
# on the VPS
chmod 644 ~/mpri-mcp/mpri-mcp/secrets/gcp-sa.json
```

`secrets/` is in `.gitignore`, so the key is never committed.

> Using another provider instead? Set `AI_PROVIDER` / `AI_MODEL` and that provider's key in
> `.env` (`OPENAI_API_KEY` + optional `OPENAI_BASE_URL`, or `ANTHROPIC_API_KEY`). You can
> switch providers later from the dashboard.

## 4. Configure

```bash
cp .env.example .env
nano .env
```

Set at least these values:

| Variable | Value |
|---|---|
| `DOMAIN` | `mpri.example.com` |
| `PUBLIC_BASE_URL` | `https://mpri.example.com`: exactly what you will give Claude, without `/mcp` |
| `GOOGLE_CLOUD_PROJECT` | your project ID |
| `GOOGLE_CLOUD_LOCATION` | `global` (the default) |
| `AI_MODEL` | `gemini-2.5-flash-lite` (the default) |

Optionally set `ADMIN_SETUP_TOKEN` to a value you choose. Otherwise a random token is
printed in the logs on first start.

## 5. Start

```bash
docker compose up -d --build
docker compose ps                         # both services "running"; mpri-mcp becomes "healthy"
docker compose logs mpri-mcp | grep -A3 "FIRST-RUN SETUP"
```

The log shows the **setup token**. The first build takes a few minutes because it downloads
Chromium.

## 6. Create the admin account

Open `https://mpri.example.com/admin`. You'll be sent to **First-run setup**. Enter the setup
token, a username and a password of at least 12 characters. This account signs in to the
dashboard and approves Claude's connection.

## 7. Check it works (recommended)

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

## 8. Connect Claude

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
| Update to the latest code | `git pull && docker compose up -d --build` (schedules and data are kept) |
| Logs | `docker compose logs -f mpri-mcp` |
| Forgot the admin password | `docker compose exec -it mpri-mcp mpri-mcp admin reset-password` |
| Crawl one supermarket from the shell | `docker compose exec mpri-mcp mpri-mcp crawl <supermarket-id>` |
| Revoke Claude's access | Dashboard → **Connected apps** → Revoke |
| Backups | Written daily (keeps 7) to `/app/data/backups` in the `mpri_data` volume. Copy them off the server: `docker compose cp mpri-mcp:/app/data/backups ./backups` |
| Restore a backup | `docker compose stop mpri-mcp`, then `docker compose cp ./backups/mpri-XXXX.db mpri-mcp:/app/data/mpri.db`, then `docker compose start mpri-mcp` |

**Keep exactly one app container.** The scheduler runs inside it, so scaling the service to
more than one replica would crawl everything twice.

### Changing the domain

1. Point the new DNS name at the VPS.
2. In `.env`, set both `DOMAIN` and `PUBLIC_BASE_URL` to the new name.
3. Run `docker compose up -d`.
4. In claude.ai, remove the connector and add it again with the new URL. Tokens are bound to the old URL, so you'll sign in again.

### Troubleshooting

| Symptom | Likely cause |
|---|---|
| Claude says "Couldn't reach the MCP server" | `PUBLIC_BASE_URL` doesn't exactly match the URL you entered (scheme, host, no trailing slash), or DNS/HTTPS isn't working yet. Open `https://<domain>/.well-known/oauth-protected-resource/mcp` in a browser: its `resource` must equal your connector URL. |
| `421 Invalid Host header` | The request's host isn't the host in `PUBLIC_BASE_URL`. |
| Caddy logs certificate errors | The DNS A record isn't pointing at this server yet, or port 80/443 is blocked. |
| Every crawl fails with `HTTP 403` or a captcha | The site blocks the VPS IP. Add a `proxy` in that supermarket's crawl settings. |
| Crawls fail with an AI provider error | Dashboard → AI shows each provider's status. Check its credentials in `.env`, then run `docker compose up -d`. |
| `/admin/setup` returns 404 | An admin already exists. Use `admin reset-password` (above). |
