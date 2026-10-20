# TÀI LIỆU YÊU CẦU DỰ ÁN: MARKET PRICING MCP SERVER

> Original requirements document supplied by the project owner (2026-10-20).
> Decisions taken after this document was written are recorded in
> [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) — notably: Gemini 1.5 Flash has been
> retired, so the model is configurable behind a provider adapter.

## 1. Yêu cầu về Quy trình Phát triển (Coding Workflow cho AI)
Trước khi bắt đầu viết code cho bất kỳ module nào, **AI bắt buộc phải cung cấp một kế hoạch triển khai (Implementation Plan) chi tiết từng bước**.
Kế hoạch này cần liệt kê rõ:
*   Cấu trúc thư mục dự kiến.
*   Các file sẽ được tạo hoặc chỉnh sửa.
*   Trình tự implement (ví dụ: Setup project -> Scheduler module -> Crawler logic -> MCP Tools -> Integration).
*   *Chỉ khi người dùng (User) đồng ý với kế hoạch này, AI mới được phép tiến hành viết code.*

## 2. Tổng quan dự án (Project Overview)
Xây dựng một hệ thống theo dõi giá sản phẩm từ các siêu thị khác nhau (Go!, Winmart, Coopmart, v.v.). Hệ thống cung cấp một **Model Context Protocol (MCP) Server** cho phép AI Agent (Claude) có khả năng:
1.  Đọc và phân tích lịch sử giá sản phẩm, báo cáo giảm giá.
2.  Quản lý cấu hình (thêm siêu thị, thêm sản phẩm và URL) trực tiếp qua giao diện chat của Claude.
3.  Cấu hình và quản lý lịch cào dữ liệu (Cron task) độc lập cho từng hệ thống siêu thị riêng biệt.

## 3. Tech Stack (Công nghệ sử dụng)
*   **Ngôn ngữ:** Python 3.10+
*   **MCP Server:** `mcp` (Official Python MCP SDK của Anthropic).
*   **Crawler:** `Playwright` (Async API) để render Web/JS + `google-genai` (gọi Gemini 1.5 Flash để extract giá thành định dạng JSON).
*   **Scheduler/Cron:** Cần sử dụng thư viện hỗ trợ Dynamic Job Scheduling (ví dụ: `APScheduler` hoặc `Celery` + `Celery Beat` hoặc `RQ`) để có thể thêm/sửa cron task động lúc runtime.

## 4. Yêu cầu tính năng (Feature Requirements)

### 4.1. Module Crawler & Dynamic Scheduler (Cào dữ liệu & Lịch trình động)
*   **Cron Task Độc lập:** **Không** sử dụng một job duy nhất để quét tất cả dữ liệu. Mỗi một siêu thị (Market) sẽ có một Cron Task độc lập. Việc này giúp:
    *   Tránh rủi ro một trang web bị lỗi timeout làm treo toàn bộ tiến trình quét của các siêu thị khác.
    *   Tối ưu hóa thời điểm quét (ví dụ: siêu thị A cập nhật giá lúc 00:00, siêu thị B cập nhật lúc 06:00).
*   **Luồng Crawler cơ bản:** Khi một Cron task của siêu thị A được trigger -> Lấy danh sách URL sản phẩm thuộc siêu thị A -> Dùng Playwright quét HTML -> Dùng AI Model parse ra `original_price`, `current_price`, `member_price` -> Lưu xuống Database lịch sử giá.

### 4.2. Khai báo MCP Tools (API cho AI Agent)
Hệ thống cần register các tool sau để publish cho Claude:

#### Nhóm Tool Quản lý & Cấu hình (Setup/Config)
1.  **`get_supermarkets`**
    *   *Mô tả:* Lấy danh sách tất cả các siêu thị đã được cấu hình trong hệ thống (kèm thông tin ID, tên, và trạng thái Cron task).
    *   *Input:* None
2.  **`add_supermarket`**
    *   *Mô tả:* Thêm một siêu thị mới vào hệ thống theo dõi.
    *   *Input:* `name` (string), `base_url` (string).
3.  **`setup_supermarket_cron_task`**
    *   *Mô tả:* Khởi tạo hoặc cập nhật lịch trình chạy quét dữ liệu (Cron schedule) cho một siêu thị cụ thể. Tool này sẽ gọi vào Scheduler Engine (như APScheduler) để đăng ký một job chạy ngầm mới mà không cần restart server.
    *   *Input:* `supermarket_id` (string), `cron_expression` (string, ví dụ: `"0 2 * * *"` để chạy lúc 2h sáng).
4.  **`add_product_with_urls`**
    *   *Mô tả:* Thêm một sản phẩm mới, kèm theo danh sách URL của sản phẩm đó tại các siêu thị hiện có.
    *   *Input Schema:*
        ```json
        {
          "name": "string",
          "category": "string (optional)",
          "urls": [
            {
              "supermarket_id": "string",
              "url": "string"
            }
          ]
        }
        ```

#### Nhóm Tool Truy vấn & Phân tích (Analytics)
5.  **`get_product_price_comparison`**
    *   *Mô tả:* Lấy giá mới nhất của một sản phẩm tại các siêu thị khác nhau để so sánh.
    *   *Input:* `product_name` (string, hỗ trợ tìm kiếm mờ).
6.  **`get_deep_discount_items`**
    *   *Mô tả:* Tìm các sản phẩm đang có mức giảm giá sâu trong khoảng thời gian gần nhất.
    *   *Input:* `min_discount_percentage` (integer).

## 5. Ví dụ Kịch bản tương tác (User Story với Claude)

*   **User:** *"Hãy thêm mới siêu thị Lotte Mart với domain lottemart.vn."*
    *   **Claude:** Dùng tool `add_supermarket` -> *"Đã thêm Lotte Mart thành công với ID là abc-xyz."*
*   **User:** *"Thiết lập lịch quét dữ liệu cho Lotte Mart vào lúc 3h sáng mỗi ngày giúp tôi."*
    *   **Claude:** Dùng tool `setup_supermarket_cron_task` truyền vào ID của Lotte Mart và cron expression `"0 3 * * *"` -> *"Đã tạo thành công một Cron Task độc lập cho Lotte Mart chạy vào 03:00 hàng ngày."*
*   **User:** *"Tôi muốn theo dõi sản phẩm 'Thùng Sữa TH True Milk Ít Đường 110ml'. Link ở Winmart là [link_1], ở Lotte Mart là [link_2]. Hãy thêm vào hệ thống."*
    *   **Claude:** Dùng tool `add_product_with_urls` -> *"Đã thiết lập theo dõi giá cho sản phẩm sữa TH True Milk. Hệ thống sẽ tự động lấy giá vào các khung giờ đã được thiết lập riêng."*
