<!-- TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host -->
# MPriMCP (tiếng Việt)

MPriMCP là MCP server tự chạy trên máy bạn, dùng để theo dõi giá sản phẩm ở các siêu thị Việt Nam
(VND). Bạn cho AI biết mình hay mua ở siêu thị nào, theo dõi sản phẩm gì và muốn được báo khi giá
dưới bao nhiêu. AI đọc giá bằng công cụ trình duyệt của nó; MPriMCP lưu giá, so sánh giữa các siêu
thị, giữ lịch sử giá và nhắn Telegram/Discord khi giá xuống dưới mốc. Máy chủ không bao giờ tự truy
cập website siêu thị. Chỉ một file chạy, kèm SQLite.

Bản tiếng Anh đầy đủ: [README.md](README.md).

## Cần gì

- Một AI client hỗ trợ MCP (Claude Desktop, Claude Code, Codex, Kimi CLI, Gemini CLI, Cursor, VS Code…).
- Một công cụ trình duyệt cho client đó: Claude in Chrome, Playwright MCP, Chrome DevTools MCP…

## Cài đặt và kết nối

1. Tải file cho hệ điều hành của bạn ở [GitHub Releases](https://github.com/tan-mai/mpri-mcp/releases),
   hoặc `cargo install --git https://github.com/tan-mai/mpri-mcp mprimcp`.
2. Khai báo với AI client:
   - Claude Desktop: `"mprimcp": { "command": "/duong-dan/mprimcp", "args": ["stdio"] }`
   - Claude Code: `claude mcp add mprimcp -- /duong-dan/mprimcp stdio`
   - Codex / Kimi / Gemini / Cursor / VS Code: xem mẫu trong [README.md](README.md#connect-your-ai-client),
     nhớ thêm một browser MCP.
3. Bật công cụ trình duyệt (Claude in Chrome…).
4. Gõ **`init`**. AI dẫn từng bước: siêu thị → sản phẩm → mốc giá → kênh thông báo → lịch → crawl đầu tiên.
   Gõ `init` lần nữa để thêm hoặc sửa; bước đã xong sẽ được bỏ qua.

Dùng hằng ngày: *"chạy crawl"*, *"so sánh giá X"*, *"báo tôi khi Y dưới 70.000đ"*.

Dữ liệu nằm ở `%APPDATA%\mprimcp` (Windows), `~/.local/share/mprimcp` (Linux),
`~/Library/Application Support/mprimcp` (macOS); đổi bằng `MPRIMCP_DATA_DIR`.

## Tự động kiểm tra giá

- Trong AI client: tạo scheduled task với câu lệnh *"Call trigger_crawl_now for every MPriMCP
  supermarket, then run MPriMCP Workflow A until no pending tasks remain."*
- Trên máy chủ: `MPRIMCP_CRON_ENABLED=true mprimcp serve` sẽ xếp task theo lịch của từng siêu thị;
  AI đọc giá ở lần chạy Workflow A kế tiếp.

## Thông báo Telegram / Discord

- Telegram: nhắn [@BotFather](https://t.me/BotFather) lệnh `/newbot`, lấy token. Nhắn một tin bất kỳ
  cho bot, mở `https://api.telegram.org/bot<TOKEN>/getUpdates` để lấy `chat.id`.
- Discord: Channel settings → Integrations → Webhooks → Copy Webhook URL.
- Nhờ AI cài (`set_notification_channel` gửi tin test ngay) hoặc vào dashboard → Thông báo. Token chỉ
  hiển thị dạng `123456***xyz`.

## Dashboard và tài khoản

`mprimcp dashboard` rồi mở link in ra. Lần đầu sẽ hỏi tên đăng nhập và mật khẩu (tối thiểu 12 ký tự).
Quên mật khẩu: `mprimcp user set-password <tên>`. Chế độ `stdio` không cần tài khoản.

## Chạy trên máy chủ riêng

`mprimcp serve` mở MCP tại `/mcp` (OAuth 2.1), dashboard và cron. Đặt sau reverse proxy HTTPS, cấu
hình `MPRIMCP_PUBLIC_URL`, `MPRIMCP_ADMIN_USER`/`MPRIMCP_ADMIN_PASSWORD` (Docker/systemd).
Hướng dẫn chi tiết: [mpri-mcp/deploy/VPS_SETUP.md](mpri-mcp/deploy/VPS_SETUP.md).

## Lưu ý

Bạn tự chịu trách nhiệm tuân thủ điều khoản sử dụng của các website siêu thị.

## Giấy phép

[MIT](LICENSE)
