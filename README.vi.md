<!-- TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host -->
<p align="center"><img src="mpri-mcp/assets/static/avatar-192.png" width="96" alt=""></p>

# MPriMCP

[![CI](https://github.com/tan-mai/mpri-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/tan-mai/mpri-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Bản tiếng Anh: [README.md](README.md)

MPriMCP là một máy chủ [MCP](https://modelcontextprotocol.io) tự cài đặt, dùng để theo dõi giá sản
phẩm ở siêu thị, đơn vị VND hoặc USD. Bạn cho AI biết mình hay mua ở siêu thị nào, muốn theo
dõi sản phẩm gì và muốn được báo khi giá xuống dưới bao nhiêu. AI đọc giá bằng công cụ trình duyệt
của nó; MPriMCP lưu giá, so sánh giữa các siêu thị, giữ lịch sử giá và nhắn tin cho bạn qua Telegram
hoặc Discord khi giá xuống dưới mốc. Bản thân máy chủ không bao giờ truy cập website của siêu thị.
Toàn bộ chỉ là một tệp chạy nhỏ, kèm cơ sở dữ liệu SQLite bên trong.

```text
Bạn:  init
AI:   Tiền tệ VND, tiếng Việt, giờ Asia/Ho_Chi_Minh: đúng chưa? …
AI:   Bạn hay mua ở siêu thị nào? …  (thêm hai siêu thị và tìm trang tìm kiếm của từng nơi)
AI:   Bạn muốn theo dõi sản phẩm gì? …  (tìm trên cả hai trang, liệt kê các quy cách và giá)
AI:   Báo khi giá dưới bao nhiêu? … Telegram hay Discord? … Kiểm tra giá vào lúc nào?
Bạn:  so sánh giá sữa TH True Milk 110ml
```

## Yêu cầu

- Một AI client hỗ trợ MCP (Claude Desktop, Claude Code, Codex, Kimi CLI, Gemini CLI, Cursor,
  VS Code, …).
- Một công cụ trình duyệt cho client đó: Claude in Chrome, [Playwright MCP](https://github.com/microsoft/playwright-mcp),
  [Chrome DevTools MCP](https://github.com/ChromeDevTools/chrome-devtools-mcp), …

## Cài đặt

Tải tệp nén cho hệ điều hành của bạn ở [GitHub Releases](https://github.com/tan-mai/mpri-mcp/releases)
(Linux x86_64/arm64, macOS Intel/Apple Silicon, Windows x86_64) rồi đặt `mprimcp` vào một thư mục
nằm trong `PATH`. Hoặc tự biên dịch:

```bash
cargo install --git https://github.com/tan-mai/mpri-mcp mprimcp
```

Dữ liệu được lưu ở `~/.local/share/mprimcp` (Linux), `~/Library/Application Support/mprimcp` (macOS)
hoặc `%APPDATA%\mprimcp` (Windows); đặt biến `MPRIMCP_DATA_DIR` để dùng thư mục khác. Cơ sở dữ liệu
ban đầu trống.

## Kết nối AI client

MPriMCP chạy như một máy chủ stdio cục bộ (`mprimcp stdio`). Nếu client chưa có sẵn công cụ trình
duyệt, hãy khai báo thêm một MCP trình duyệt bên cạnh.

**Claude Desktop** (`claude_desktop_config.json`), sau đó bật Claude in Chrome:

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

**Gemini CLI** (`~/.gemini/settings.json`), **Kimi CLI** và **Cursor** (`~/.cursor/mcp.json`) dùng
cùng cấu trúc `mcpServers`:

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

Thay `/path/to/mprimcp` bằng đường dẫn thật tới tệp `mprimcp` trên máy bạn. Các client bỏ qua phần
hướng dẫn của máy chủ MCP (Codex, Kimi, Gemini CLI, …) nên đọc [`AGENTS.md`](AGENTS.md): tệp này
chứa cùng các quy trình làm việc.

## Bắt đầu: gõ `init`

Gõ `init` (hoặc chọn prompt `init`). AI sẽ dẫn bạn thiết lập từng bước: tiền tệ, ngôn ngữ và múi
giờ, siêu thị, sản phẩm, mốc giá, kênh thông báo, lịch kiểm tra và lần kiểm tra giá đầu tiên. Có thể gõ lại bất cứ lúc nào để thêm
hoặc sửa; những bước đã xong sẽ được bỏ qua.

Dùng hằng ngày: *"chạy crawl"*, *"so sánh giá X"*, *"báo tôi khi Y dưới 70.000đ"*,
*"món nào đang giảm giá sâu?"*.

## Tự động hoá

Giá do AI đọc, nên muốn kiểm tra giá định kỳ thì cần một tác vụ AI định kỳ:

- **Trong AI client** (nên dùng khi chạy trên máy tính cá nhân): tạo một tác vụ định kỳ với câu lệnh
  *"Gọi trigger_crawl_now cho mọi siêu thị trong MPriMCP, sau đó chạy MPriMCP Workflow A cho đến khi
  không còn task nào đang chờ."* Máy tính phải bật và trình duyệt phải sẵn sàng.
- **Trên máy chủ**: `MPRIMCP_CRON_ENABLED=true mprimcp serve` sẽ xếp task quét giá theo lịch của từng
  siêu thị (cron 5 trường, theo múi giờ của máy chủ); AI sẽ đọc giá ở lần chạy Workflow A kế tiếp. Lịch đặt bằng
  `setup_supermarket_cron_task` luôn được lưu, kể cả khi cron trên máy chủ đang tắt.

## Thông báo Telegram / Discord

Máy chủ nhắn tin cho bạn ngay khi một giá vừa lưu thấp hơn mốc, và sau mỗi lượt quét giá với danh sách
nơi bán rẻ nhất của từng sản phẩm. Hãy nhờ AI thiết lập (`set_notification_channel` gửi tin thử ngay)
hoặc vào trang quản trị.

- **Telegram:** nhắn cho [@BotFather](https://t.me/BotFather) lệnh `/newbot` và sao chép token
  (`123456789:AA…`). Gửi một tin bất kỳ cho bot mới, rồi mở
  `https://api.telegram.org/bot<TOKEN>/getUpdates` và lấy giá trị `chat.id`. Đó là Chat ID của bạn;
  đừng dùng dãy số ở đầu token vì đó là ID của chính bot.
- **Discord:** Cài đặt kênh → Tích hợp → Webhook → Webhook mới → Sao chép URL Webhook.

Token được lưu trong cơ sở dữ liệu cục bộ và không bao giờ hiển thị đầy đủ (chỉ dạng `123456***xyz`).

## Trang quản trị

```bash
mprimcp dashboard          # in ra http://127.0.0.1:8765/admin
```

Lần chạy đầu sẽ hỏi tên đăng nhập và mật khẩu (tối thiểu 12 ký tự, lưu dưới dạng mã băm argon2id).
Các màn hình: tổng quan, siêu thị (bật/tắt lịch), sản phẩm và giá mới nhất, giá cần duyệt, mốc giá,
thông báo, và cài đặt tài khoản (tiền tệ, ngôn ngữ, múi giờ). Trang quản trị và tin nhắn
Telegram/Discord có tiếng Anh và tiếng Việt. Quên mật khẩu: `mprimcp user set-password <tên-đăng-nhập>`. Chế độ `stdio` không cần
tài khoản.

## Tự cài trên máy chủ riêng

`mprimcp serve` mở MCP qua Streamable HTTP tại `/mcp` (OAuth 2.1 có đăng ký client tự động: client
của bạn sẽ mở trang đăng nhập của máy chủ), kèm trang quản trị và cron tuỳ chọn. Hãy đặt máy chủ sau
một proxy ngược có HTTPS và cấu hình:

| Biến | Tác dụng |
|---|---|
| `MPRIMCP_BIND` | Địa chỉ lắng nghe (mặc định `127.0.0.1:8765`) |
| `MPRIMCP_PUBLIC_URL` | Địa chỉ công khai, ví dụ `https://mpri.example.com` (URL connector = `…/mcp`) |
| `MPRIMCP_ADMIN_USER`, `MPRIMCP_ADMIN_PASSWORD` | Tạo tài khoản đầu tiên khi khởi động không có terminal (Docker, systemd); bị bỏ qua khi đã có tài khoản |
| `MPRIMCP_CRON_ENABLED` | `true` để xếp task quét giá theo lịch của từng siêu thị |
| `MPRIMCP_TRUST_PROXY` | `true` khi chạy sau proxy, để giới hạn đăng nhập sai nhận đúng IP người dùng |
| `MPRIMCP_OAUTH_REDIRECT_URIS` | Thêm các địa chỉ chuyển hướng OAuth được phép (địa chỉ của Claude và loopback có sẵn) |
| `MPRIMCP_CURRENCY` | `VND` (mặc định) hoặc `USD`; cố định cài đặt (chỉ đổi được khi chưa có giá nào) |
| `MPRIMCP_LANGUAGE` | `en` (mặc định) hoặc `vi` cho trang quản trị và thông báo; cố định cài đặt |
| `MPRIMCP_TIMEZONE` | Múi giờ IANA cho lịch và thời gian hiển thị (mặc định `Asia/Ho_Chi_Minh`); cố định cài đặt |
| `MPRIMCP_DATA_DIR`, `RUST_LOG` | Thư mục dữ liệu, mức ghi log |

Docker image, tệp compose kèm Caddy và tệp systemd: [`mpri-mcp/deploy/`](mpri-mcp/deploy/)
([hướng dẫn](mpri-mcp/deploy/VPS_SETUP.md)). Toàn bộ cấu hình: [`mpri-mcp/.env.example`](mpri-mcp/.env.example).

```text
mprimcp stdio | serve | dashboard | import --from <sqlite-file> | user create | user set-password <username> | version
```

## Tiền tệ, ngôn ngữ và múi giờ

Cách thu thập giá dùng được với mọi cửa hàng trực tuyến: AI mở trang sản phẩm và đọc giá, còn máy chủ
không bao giờ truy cập cửa hàng. Mỗi máy chủ dùng một tiền tệ, chọn trong lúc `init`
(`set_preferences`), ở trang tài khoản của trang quản trị hoặc bằng `MPRIMCP_CURRENCY`:

- **VND**: số nguyên (`45.900đ` → `45900`).
- **USD**: có phần lẻ (`$3.99` → `3.99`).

Tiền tệ chỉ đổi được khi chưa có giá nào được lưu. Ngôn ngữ (`en`/`vi`) và múi giờ đổi được bất cứ
lúc nào; lịch kiểm tra đi theo múi giờ.

Chưa hỗ trợ: các tiền tệ khác, và tên sản phẩm chỉ viết bằng hệ chữ không phải Latin (Kirin, Trung,
Nhật, Thái, …). Rất hoan nghênh đóng góp.

## Câu hỏi thường gặp và xử lý sự cố

- **AI không thấy các tool.** Kiểm tra đường dẫn trong cấu hình client và chạy
  `/path/to/mprimcp version` trong terminal. Khởi động lại client sau khi sửa cấu hình.
- **AI báo "không có công cụ trình duyệt".** Bật Claude in Chrome, hoặc thêm Playwright MCP / Chrome
  DevTools MCP.
- **Một giá bị đánh dấu `needs_review`.** Giá lệch hơn 50% so với lần trước; hãy chấp nhận hoặc loại
  bỏ ở trang duyệt giá của trang quản trị (`/admin/review`).
- **Gửi tin thử Telegram báo "the bot can't send messages to the bot".** Chat ID đang là ID của chính
  bot; hãy dùng `chat.id` lấy từ `getUpdates` (xem ở trên).
- **Lỗi "database is locked".** `stdio` và `dashboard` chạy cùng lúc được (SQLite WAL); hãy chắc chắn
  không có chương trình nào khác giữ tệp và chỉ một tiến trình `serve` dùng mỗi thư mục dữ liệu.
- **Một trang web chặn máy chủ của bạn.** Một số siêu thị chặn IP của trung tâm dữ liệu. Giá do trình
  duyệt của AI đọc, nên điều này chỉ ảnh hưởng khi trình duyệt đó chạy trên máy chủ.

## Lưu ý

Bạn tự chịu trách nhiệm sử dụng website của các siêu thị đúng theo điều khoản sử dụng của họ. MPriMCP
chỉ lưu những gì AI của bạn đọc được.

## Giấy phép

[MIT](LICENSE) © 2026 Tan Mai
