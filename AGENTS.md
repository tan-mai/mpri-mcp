# AGENTS.md — MPriMCP

Instructions for AI agents (Codex, Kimi CLI, Gemini CLI, Cursor, …) that use the **MPriMCP** MCP server
but do not read MCP server instructions. Claude and other clients receive the same text from the server
during `initialize`; the block below is kept identical to it by a test (`mpri-mcp/tests/system.rs`).

You also need a browser tool (Claude in Chrome, Playwright MCP, Chrome DevTools MCP, …): MPriMCP never
fetches web pages itself.

To start onboarding, the user types `init` (or picks the `init` prompt, which says):

```text
Run MPriMCP Workflow 0 (ONBOARDING) from the server instructions.
Start by calling get_setup_status, then guide me step by step: supermarkets,
products, price targets, notifications, schedule, first crawl.
Ask one question at a time and wait for my answer before saving anything.
```

## Server instructions

```text
Tracks product prices at supermarkets. Prices are in the server's currency (VND or USD,
shown by get_setup_status and in every price answer).
The server never fetches web pages. YOU read prices with your browser tool
(e.g. Claude in Chrome, Playwright MCP, Chrome DevTools MCP). If you have no
browser tool, say so and stop before Workflow A or B.

WORKFLOW 0 - ONBOARDING (when the user picks the "init" prompt, says
"init" / "khởi tạo" / "setup", or get_supermarkets is empty)
Speak the user's language. Ask one step at a time; every step except 1 can be skipped.
1. Call get_setup_status. Tell the user briefly what is already set up and
   continue from next_step. If no price is stored yet (has_any_price=false),
   confirm the currency (VND or USD), the language of the dashboard and the
   Telegram/Discord messages (en or vi) and the timezone, and call
   set_preferences for anything that differs. If everything is done, ask what
   they want to add or change.
2. Supermarkets: ask which supermarkets they shop at. For each one:
   - If the user pasted a URL, open it. Otherwise find the official online store
     with your browser and show the user the site you found.
   - Search the site for a common keyword (e.g. "milk" / "sữa") and copy the search
     results URL. Replace the keyword with {q} to get search_url_template.
   - Show name, base_url and search_url_template. WAIT for confirmation,
     then call add_supermarket (do not pass an id; the server creates it).
3. Products: ask which products to track. Run Workflow B for each one.
4. Price targets: for each new product, ask below which price they want to be
   told and call set_price_alert. Skip if the user doesn't want one.
5. Notifications: ask whether they want Telegram or Discord messages.
   - Telegram: they need a bot token (from @BotFather) and their own chat_id
     (from getUpdates, not the number at the start of the token).
   - Discord: they need a channel webhook URL.
   Call set_notification_channel and report test_ok. Never repeat the token back
   in full.
6. Schedule: ask when to check prices (supermarkets often discount at the
   weekend). Call setup_supermarket_cron_task for each supermarket.
   If the result says cron_enabled=false, help the user create a scheduled task
   in their AI client with this prompt:
   "Call trigger_crawl_now for every MPriMCP supermarket, then run MPriMCP
   Workflow A until no pending tasks remain."
7. First crawl: offer trigger_crawl_now for all supermarkets, then run Workflow A.
8. Finish with a short summary and example requests in the user's language, e.g.
   "run the crawl", "compare prices of <product>", "tell me when <product> drops below <price>".

SETUP (individual tools)
get_supermarkets, add_supermarket, update_supermarket, setup_supermarket_cron_task
(standard cron, server timezone), add_product_with_urls, get_search_urls,
set_notification_channel, set_preferences, get_setup_status.
Supermarkets can be referred to by name or supermarket_id.
Product names are matched fuzzily and without accents. A product has no price until
its supermarket has been crawled at least once.

ANALYSIS
get_product_price_comparison, get_price_history, get_deep_discount_items.
Prices flagged needs_review=true are unverified: mention the flag when you show them.

WORKFLOW A - AGENT CRAWL (when asked to run the crawl, or when pending_tasks > 0)
1. Call get_pending_crawl_tasks (optionally per supermarket).
2. For each task, open task.url with your browser tool.
   Read the price ONLY for the product on that page. Ignore related products,
   combo/bundle prices and "frequently bought together" blocks.
3. Record:
   - price: the price a regular customer pays now for this exact item.
   - original_price: the struck-through / pre-discount price, if shown.
   - member_price: a separate member/loyalty price, if shown.
   - raw_text: the short page text the price was read from.
   Use status out_of_stock if the page says sold out / "hết hàng",
   not_found if the page is gone or shows a different product,
   error if the page did not load or you could not read a price.
   Never guess a price. Send prices as numbers in the server's currency:
   VND has no decimals ("45.900đ" -> 45900), USD has cents ("$3.99" -> 3.99).
   For multi-buy offers ("2 for $5") or prices per weight, record the price of
   one item as sold.
4. Call submit_price_results in batches of up to 20, before lease_expires_at.
5. Repeat until remaining_pending is 0. Then summarise: ok / flagged / failed
   per supermarket, and list any discounts you saw. Mention alerts_triggered; the
   server already messaged the user about them and sent the cheapest-price summary
   (summary_sent_to), so do not send those again.

PRICE ALERTS AND NOTIFICATIONS
When the user wants to know when a product gets cheaper than a price ("tell me when
... drops below $3" / "báo tôi khi ... dưới 70.000đ"), call
set_price_alert(product_name, target_price); confirm the matched product name in the
answer. get_price_alerts lists alerts, delete_price_alert removes one. Notification
channels are set with set_notification_channel (or the dashboard). The server
messages the user at once when a saved price is below a target, and after every
finished agent crawl with where each tracked product is cheapest.

WORKFLOW B - ADD A PRODUCT BY SEARCH (when the user wants to track a product by name)
1. Call get_supermarkets, then get_search_urls(keyword) for the chosen supermarkets
   (default: all).
2. Open each search_url with your browser tool (if null, open base_url and use the
   site's search box). Collect the top 3-5 relevant results per supermarket:
   name, pack size/quantity, current price, product page URL.
3. Group results by IDENTICAL SPEC (brand, variant, volume/weight, pack count,
   e.g. "case of 48 x 110ml" vs "pack of 4 x 110ml" are DIFFERENT products).
   Present the groups to the user with numbers, showing which supermarkets carry
   each one and their prices. Point out spec differences explicitly.
4. WAIT for the user's confirmation. Never save anything without it.
   The user may pick one or more groups, or exclude some supermarkets.
5. For each confirmed group, call add_product_with_urls once with:
   - name: standard form "<Brand> <Product> <Variant> <Size> <Pack>",
     e.g. "TH True Milk Less Sugar 110ml Case 48".
   - category if known.
   - one product page URL per supermarket (never a search-results URL).
   Only put URLs of the same spec into one product.
6. Offer trigger_crawl_now for the affected supermarkets so prices appear.
```
