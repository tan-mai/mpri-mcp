<!-- TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host -->
# Tool parity: Python release (`python-final`) → Rust

Reference: tag `python-final` (commit `2a17be6`), `src/mpri_mcp/tools/*.py`. "Kiểm" = covered by the
named test in `tests/` (MCP calls through rmcp over an in-process transport) or by a manual check.
Times are shown as in Python (`Sun 22/11/2026 21:20`, Asia/Ho_Chi_Minh). Fields marked *drop-if-null*
are omitted when empty, exactly like Python's `drop_none`.

## Intentional differences (all additive, required by the BR)

| # | Where | Python | Rust |
|---|---|---|---|
| D1 | every tool taking `supermarket_id` / `supermarket_ids` | ID only | ID **or name** (accent/case-insensitive); unknown → same error as before |
| D2 | `add_supermarket` output | `id` | `id` **and** `supermarket_id` (same value) |
| D3 | duplicate supermarket names | `normalize_name` (punctuation folded) | accent/case-insensitive name (BR FR-5, AC-7): "Co.op Mart" vs "Co-op Mart" are two names → slugs `co-op-mart`, `co-op-mart-2` |
| D4 | `setup_supermarket_cron_task` output | — | `cron_enabled`, and `hint` when the server cron is off (FR-4) |
| D5 | `get_supermarkets.schedule.next_run` | APScheduler next run | next run when this process runs the cron (`serve` + `MPRIMCP_CRON_ENABLED=true`), else `null` |
| D6 | `submit_price_results.notifications` text | "…(dashboard → Notifications)…" | "…(set_notification_channel or dashboard → Thông báo)…" |
| D7 | tool descriptions | "the browser (Claude in Chrome / Claude Browser)" | "your browser tool" (FR-9) |
| D8 | invalid argument types | pydantic error inside the tool result | JSON-RPC `invalid_params` error (rmcp); numbers given as strings and lists given as JSON strings are still accepted, like the Python SDK |
| D9 | `outputSchema` | generic object | typed schema per tool (schemars) |
| D10 | new tools | — | `get_setup_status`, `set_notification_channel` (FR-6, FR-7); prompt `init` (FR-8) |
| D11 | money in inputs/outputs | integer VND | number in the server currency: VND stays an integer (unchanged output), USD has 2 decimals; stored as integer minor units. `currency` field reports VND or USD |
| D12 | preferences | — | `set_preferences` tool; `get_setup_status` adds `currency`, `language`, `timezone`; times follow the configured timezone (default `Asia/Ho_Chi_Minh`) |

## Setup and schedule

| Tool | Parameters (default) | Output | Validation / errors | Kiểm |
|---|---|---|---|---|
| `get_supermarkets` | — | `supermarkets[]{id, name, base_url, schedule{cron_expression, enabled, timezone, next_run, crawl_in_progress}, tracked_urls, failing_urls, search_url_template, pending_tasks, needs_review_count, last_run{status, trigger, started_at, urls_ok, urls_failed, error*}\|null}`, `timezone` | runs task maintenance first | ✅ `ac02_ac09_empty_database`, `ac08_…`, `old_ac06_to_ac11_…` |
| `add_supermarket` | `name`, `base_url`, `search_url_template` (null) | `id`, `supermarket_id`, `name`, `base_url`, `search_url_template`, `message` | name required/≤200; base_url http(s) with a dot; template: one `{q}`, ≤500, full URL; duplicate → "Supermarket 'X' already exists with ID 'y'." | ✅ `ac05_ac06_ac07_slug_and_duplicates` |
| `update_supermarket` | `supermarket_id`, `name` (null), `base_url` (null), `search_url_template` (null, "" removes) | `id`, `name`, `base_url`, `search_url_template`, `updated[]` (sorted), `message` | nothing passed → "Nothing to update: pass name, base_url or search_url_template."; other name clash | ✅ `old_ac12_ac13_search_templates` |
| `get_search_urls` | `keyword`, `supermarket_ids` (null = all) | `keyword` (collapsed), `supermarkets[]{supermarket_id, name, base_url, search_url, note}` | keyword required/≤200; unknown IDs listed; keyword URL-encoded like `quote(safe="")` | ✅ `old_ac12_ac13_search_templates` |
| `setup_supermarket_cron_task` | `supermarket_id`, `cron_expression` | `supermarket_id`, `supermarket_name`, `cron_expression`, `timezone`, `next_runs[3]`, `message`, `cron_enabled`, `hint*` | "Invalid cron expression: …" with APScheduler's messages; 0/7 = Sunday; macros; day AND weekday | ✅ `ac14_…`, `cron::tests::matches_python_reference` (90 cases from APScheduler) |
| `disable_supermarket_cron_task` | `supermarket_id` | `supermarket_id`, `cron_expression`, `enabled: false`, `message` | — | ✅ `ac14_…` |
| `trigger_crawl_now` | `supermarket_id` | `supermarket_id`, `started`, `queued_tasks`, `already_open`, `pending_tasks`, `message` | no active URL → "Supermarket 'x' has no active product URLs to crawl yet."; never queues a URL twice | ✅ `ac08_supermarket_by_name_and_old_ac03_ac04_queue` |
| `add_product_with_urls` | `name`, `urls[]{supermarket_id, url}`, `category` (null) | `product_id`, `name`, `category`, `created`, `urls[]{supermarket_id, url, status added\|updated\|unchanged\|error, error*, warning*}`, `message` | name ≤300 with letters/digits; same name ignoring accents/case reuses the product; host mismatch → warning | ✅ `lenient_arguments_like_python_sdk`, seeding in all tests |

## Analysis

| Tool | Parameters (default) | Output | Validation | Kiểm |
|---|---|---|---|---|
| `get_product_price_comparison` | `product_name` | `product{id, name, category}`, `currency`, `prices[]` (cheapest first), `cheapest\|null`, `other_matches[]` (within 5 points) | fuzzy: rapidfuzz `token_set_ratio` ≥60, tiebreak `token_sort_ratio` | ✅ `old_ac06_…`, `ac22_…`, `text::tests::matches_rapidfuzz_reference` (391 pairs) |
| price row | — | `supermarket_id`, `supermarket_name`, `current_price*`, `original_price*`, `member_price*`, `discount_pct*`, `member_discount_pct*`, `in_stock*`, `checked_at*`, `stale*` (>3 days), `no_data_yet*`, `last_error*`, `url`, `source*`, `needs_review*` | discount rounded to 0.1 like Python `round` | ✅ `ac22_…` (11.3 / 13.6) |
| `get_deep_discount_items` | `min_discount_percentage`, `days` (7), `limit` (50) | `currency`, `count`, `items[]{product_name, category*, supermarket_id, supermarket_name, current_price, original_price*, discount_pct, member_price*, member_discount_pct*, checked_at, url}` | 1–99; days 1–365; limit clamped 1–500; `needs_review` prices left out | ✅ `old_ac08_big_change_is_flagged_and_hidden_from_discounts` |
| `get_price_history` | `product_name`, `days` (30), `supermarket_id` (null) | `product{id, name}`, `days`, `currency`, `series[]{supermarket_id, supermarket_name, points[]{date, current_price, original_price*, member_price*, needs_review*}, stats{min_price, min_date, max_price, avg_price, latest_price, latest_vs_min_pct}*}`, `other_matches[]` | days 1–730; last price of each local day; avg uses banker's rounding like Python | ✅ `old_ac08_…` |

## Agent crawl

| Tool | Parameters (default) | Output | Rules | Kiểm |
|---|---|---|---|---|
| `get_pending_crawl_tasks` | `supermarket_id` (null), `limit` (20) | `tasks[]{task_id, product_name, category*, supermarket_id, supermarket_name, url, last_price*, last_original_price*, last_checked_at*, lease_expires_at}`, `count`, `remaining_pending`, `currency`, `timezone` | limit 1–50; oldest first; lease 30 min; claim in `BEGIN IMMEDIATE` (safe across processes) | ✅ `old_ac05_parallel_claims_do_not_overlap`, `ac18_two_processes_claim_without_duplicates`, `old_ac09_expired_lease_goes_back_to_pending` |
| `submit_price_results` | `results[]{task_id, status ok\|out_of_stock\|not_found\|error, price, original_price, member_price, raw_text (≤500), note (≤300)}` | `accepted`, `alerts_triggered*`, `summary_sent_to*`, `notifications*`, `requeued*`, `flagged[]`, `rejected[]{task_id, reason}`, `warnings*`, `remaining_pending` | ≤50 per call; same rejection reasons as Python; >50 % change → `needs_review`; `error` re-queues up to 3 claims; tasks pending >48 h expire; `source = agent`; run closes with `trigger = agent` | ✅ `old_ac06_to_ac11_submit_lifecycle`, `old_ac08_…`, `alerts_trigger_and_summary_reported` |

## Alerts and notifications

| Tool | Parameters (default) | Output | Rules | Kiểm |
|---|---|---|---|---|
| `set_price_alert` | `product_name`, `target_price`, `supermarket_ids` (null = all) | `product_name`, `target_price`, `supermarket_ids*`, `below_target_now`, `cheapest{…}*`, `created`, `message` ("Set the alert: X below 70,000 VND.") | 0 < target ≤ 1e9; one alert per product; resets announced prices | ✅ `alerts_trigger_and_summary_reported` |
| `get_price_alerts` | — | `count`, `alerts[]`, `currency` | sorted by product name | ✅ same |
| `delete_price_alert` | `product_name` | `product_name`, `deleted: true`, `message` | fuzzy over alert products | ✅ same |
| `get_setup_status` *(new)* | — | `supermarkets{count, items[]{supermarket_id, name, has_search_url_template, has_schedule}}`, `products`, `products_without_urls`, `price_alerts`, `notification_channels[]{channel, enabled, target_masked, last_test_ok, last_test_at}`, `cron_enabled`, `mode`, `has_any_price`, `next_step` | optional steps never block later ones | ✅ `ac02_ac09_empty_database`, `ac15_…` |
| `set_notification_channel` *(new)* | `channel`, `token` ("" removes), `chat_id` (Telegram, required), `enabled` (true) | `channel`, `target_masked` (`123456***xyz`), `enabled`, `test_ok`, `error*`, `removed*`, `message` | Telegram token `<5+ digits>:<30+ chars>`; Discord webhook prefixes; test message sent at once; saved even if the test fails | ✅ `ac10_ac13_…`, `ac11_…`, `ac12_…` |
