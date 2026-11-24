// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
mod common;

use std::collections::HashSet;

use common::*;
use mprimcp::config::Mode;
use rmcp::model::GetPromptRequestParams;
use serde_json::{Value, json};

#[tokio::test]
async fn ac02_ac09_empty_database() {
    let h = harness().await;
    let out = h.call("get_supermarkets", json!({})).await;
    assert_eq!(out["supermarkets"], json!([]));
    assert_eq!(out["timezone"], "Asia/Ho_Chi_Minh");
    let status = h.call("get_setup_status", json!({})).await;
    assert_eq!(status["next_step"], "add_supermarket");
    assert_eq!(status["mode"], "stdio");
    assert_eq!(status["cron_enabled"], false);
    assert_eq!(status["supermarkets"]["count"], 0);
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM supermarkets").await, 0);
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM products").await, 0);
}

#[tokio::test]
async fn ac05_ac06_ac07_slug_and_duplicates() {
    let h = harness().await;
    let out = h.call("add_supermarket", json!({"name": "Bách Hóa Xanh", "base_url": "bachhoaxanh.com"})).await;
    assert_eq!(out["supermarket_id"], "bach-hoa-xanh");
    assert_eq!(out["id"], "bach-hoa-xanh");
    assert_eq!(out["base_url"], "https://bachhoaxanh.com");

    let err = h.call_err("add_supermarket", json!({"name": "bach hoa xanh", "base_url": "bhx.vn"})).await;
    assert!(err.contains("Bách Hóa Xanh") && err.contains("bach-hoa-xanh"), "{err}");

    let a = h.call("add_supermarket", json!({"name": "Co.op Mart", "base_url": "cooponline.vn"})).await;
    let b = h.call("add_supermarket", json!({"name": "Co-op Mart", "base_url": "coopmart.vn"})).await;
    assert_eq!(a["supermarket_id"], "co-op-mart");
    assert_eq!(b["supermarket_id"], "co-op-mart-2");
}

#[tokio::test]
async fn ac08_supermarket_by_name_and_old_ac03_ac04_queue() {
    let h = harness().await;
    let id = h.seed_market("Bách Hóa Xanh", "bachhoaxanh.com", 6).await;
    let out = h.call("trigger_crawl_now", json!({"supermarket_id": "Bách Hóa Xanh"})).await;
    assert_eq!(out["supermarket_id"], id);
    assert_eq!(out["queued_tasks"], 6);
    assert_eq!(out["started"], true);
    let markets = h.call("get_supermarkets", json!({})).await;
    assert_eq!(markets["supermarkets"][0]["pending_tasks"], 6);
    let again = h.call("trigger_crawl_now", json!({"supermarket_id": "bach hoa xanh"})).await;
    assert_eq!(again["queued_tasks"], 0);
    assert_eq!(again["already_open"], 6);
    assert_eq!(again["pending_tasks"], 6);
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM crawl_tasks").await, 6);
    let missing = h.call_err("trigger_crawl_now", json!({"supermarket_id": "khong-co"})).await;
    assert!(missing.contains("not found"), "{missing}");
}

#[tokio::test]
async fn old_ac05_parallel_claims_do_not_overlap() {
    let h = harness().await;
    h.seed_market("Lotte Mart", "lottemart.vn", 6).await;
    h.call("trigger_crawl_now", json!({"supermarket_id": "lotte-mart"})).await;
    let c2 = connect(h.app.clone()).await;
    let (a, b) = tokio::join!(
        call_raw(&h.client, "get_pending_crawl_tasks", json!({"limit": 3})),
        call_raw(&c2, "get_pending_crawl_tasks", json!({"limit": 3})),
    );
    let ids = |v: &Value| -> Vec<String> {
        v["tasks"].as_array().unwrap().iter().map(|t| t["task_id"].as_str().unwrap().to_string()).collect()
    };
    let (ia, ib) = (ids(&a.1), ids(&b.1));
    assert_eq!(ia.len() + ib.len(), 6);
    let all: HashSet<&String> = ia.iter().chain(ib.iter()).collect();
    assert_eq!(all.len(), 6);
}

async fn claim_all(h: &Harness) -> Vec<Value> {
    h.call("get_pending_crawl_tasks", json!({"limit": 50})).await["tasks"].as_array().unwrap().clone()
}

#[tokio::test]
async fn old_ac06_to_ac11_submit_lifecycle() {
    let h = harness().await;
    let id = h.seed_market("WinMart", "winmart.vn", 3).await;
    h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
    let tasks = claim_all(&h).await;
    assert_eq!(tasks.len(), 3);
    assert!(tasks[0]["lease_expires_at"].is_string());
    let t: Vec<String> = tasks.iter().map(|t| t["task_id"].as_str().unwrap().to_string()).collect();

    let out = h
        .call(
            "submit_price_results",
            json!({"results": [
                {"task_id": t[0], "status": "ok", "price": 45900, "original_price": 52000, "raw_text": "45.900đ"},
                {"task_id": t[1], "status": "ok", "price": 0},
                {"task_id": t[2], "status": "ok", "price": 50000, "original_price": 40000},
            ]}),
        )
        .await;
    assert_eq!(out["accepted"], 1);
    let rejected = out["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), 2);
    assert!(rejected[0]["reason"].as_str().unwrap().contains("greater than 0"));
    assert!(rejected[1]["reason"].as_str().unwrap().contains("lower than price"));
    assert!(out.get("notifications").is_none());

    let cmp = h.call("get_product_price_comparison", json!({"product_name": "san pham 0"})).await;
    assert_eq!(cmp["prices"][0]["current_price"], 45900);
    assert_eq!(cmp["prices"][0]["source"], "agent");
    assert_eq!(cmp["prices"][0]["discount_pct"], 11.7);
    assert_eq!(cmp["cheapest"]["supermarket_id"], id);

    let fixed = h
        .call(
            "submit_price_results",
            json!({"results": [
                {"task_id": t[1], "status": "error", "note": "page did not load"},
                {"task_id": t[2], "status": "not_found"},
            ]}),
        )
        .await;
    assert_eq!(fixed["requeued"], 1);
    assert_eq!(fixed["remaining_pending"], 1);

    for attempt in 2..=3 {
        let again = claim_all(&h).await;
        assert_eq!(again.len(), 1, "attempt {attempt}");
        h.call("submit_price_results", json!({"results": [{"task_id": t[1], "status": "error"}]})).await;
    }
    let status: String = sqlx::query_scalar("SELECT status FROM crawl_tasks WHERE id = ?")
        .bind(&t[1])
        .fetch_one(&h.app.db)
        .await
        .unwrap();
    assert_eq!(status, "failed");

    let markets = h.call("get_supermarkets", json!({})).await;
    let run = &markets["supermarkets"][0]["last_run"];
    assert_eq!(run["trigger"], "agent");
    assert_eq!(run["status"], "partial");
    assert_eq!(run["urls_ok"], 1);
    assert_eq!(run["urls_failed"], 2);
}

#[tokio::test]
async fn old_ac08_big_change_is_flagged_and_hidden_from_discounts() {
    let h = harness().await;
    let id = h.seed_market("GO", "go-vietnam.vn", 1).await;
    for (price, original) in [(100_000, 100_000), (40_000, 80_000)] {
        h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
        let task = claim_all(&h).await[0]["task_id"].as_str().unwrap().to_string();
        let out = h
            .call(
                "submit_price_results",
                json!({"results": [{"task_id": task, "status": "ok", "price": price, "original_price": original}]}),
            )
            .await;
        if price == 40_000 {
            assert_eq!(out["flagged"][0]["change_pct"], -60.0);
            assert_eq!(out["flagged"][0]["last_price"], 100_000);
        }
    }
    let cmp = h.call("get_product_price_comparison", json!({"product_name": "san pham 0"})).await;
    assert_eq!(cmp["prices"][0]["needs_review"], true);
    let deals = h.call("get_deep_discount_items", json!({"min_discount_percentage": 10})).await;
    assert_eq!(deals["count"], 0);
    let history = h.call("get_price_history", json!({"product_name": "san pham 0", "supermarket_id": "GO"})).await;
    assert_eq!(history["series"][0]["stats"]["latest_price"], 40_000);
}

#[tokio::test]
async fn old_ac09_expired_lease_goes_back_to_pending() {
    let h = harness().await;
    let id = h.seed_market("Mega", "mmvietnam.com", 2).await;
    h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
    assert_eq!(claim_all(&h).await.len(), 2);
    sqlx::query("UPDATE crawl_tasks SET lease_expires_at = 1").execute(&h.app.db).await.unwrap();
    let again = claim_all(&h).await;
    assert_eq!(again.len(), 2);
}

#[tokio::test]
async fn old_ac12_ac13_search_templates() {
    let h = harness().await;
    let id = h.seed_market("Co.op Online", "cooponline.vn", 0).await;
    h.seed_market("Lotte", "lottemart.vn", 0).await;
    let err = h
        .call_err("update_supermarket", json!({"supermarket_id": id, "search_url_template": "https://x.vn/search"}))
        .await;
    assert!(err.contains("{q}"), "{err}");
    h.call(
        "update_supermarket",
        json!({"supermarket_id": id, "search_url_template": "https://cooponline.vn/search?router=productListing&query={q}"}),
    )
    .await;
    let out = h.call("get_search_urls", json!({"keyword": "  sữa TH   ít đường "})).await;
    assert_eq!(out["keyword"], "sữa TH ít đường");
    let markets = out["supermarkets"].as_array().unwrap();
    let coop = markets.iter().find(|m| m["supermarket_id"] == id.as_str()).unwrap();
    assert_eq!(
        coop["search_url"],
        "https://cooponline.vn/search?router=productListing&query=s%E1%BB%AFa%20TH%20%C3%ADt%20%C4%91%C6%B0%E1%BB%9Dng"
    );
    let lotte = markets.iter().find(|m| m["supermarket_id"] == "lotte").unwrap();
    assert_eq!(lotte["search_url"], Value::Null);
    assert_eq!(lotte["note"], "open base_url and use the site's search box");
    let nothing = h.call_err("update_supermarket", json!({"supermarket_id": id})).await;
    assert!(nothing.contains("Nothing to update"));
}

#[tokio::test]
async fn ac14_schedule_saved_when_cron_off() {
    let h = harness().await;
    let id = h.seed_market("Bách Hóa Xanh", "bachhoaxanh.com", 1).await;
    let out = h
        .call(
            "setup_supermarket_cron_task",
            json!({"supermarket_id": "Bách Hóa Xanh", "cron_expression": "0 8 * * 6,0"}),
        )
        .await;
    assert_eq!(out["cron_enabled"], false);
    assert!(out["hint"].as_str().unwrap().contains("MPRIMCP_CRON_ENABLED=true"));
    assert_eq!(out["next_runs"].as_array().unwrap().len(), 3);
    let stored: (Option<String>, bool) =
        sqlx::query_as("SELECT cron_expression, cron_enabled FROM supermarkets WHERE id = ?")
            .bind(&id)
            .fetch_one(&h.app.db)
            .await
            .unwrap();
    assert_eq!(stored, (Some("0 8 * * 6,0".into()), true));
    let bad =
        h.call_err("setup_supermarket_cron_task", json!({"supermarket_id": id, "cron_expression": "61 * * * *"})).await;
    assert!(bad.starts_with("Invalid cron expression"), "{bad}");
    let paused = h.call("disable_supermarket_cron_task", json!({"supermarket_id": id})).await;
    assert_eq!(paused["enabled"], false);
    assert_eq!(paused["cron_expression"], "0 8 * * 6,0");
}

#[tokio::test]
async fn ac15_cron_queues_tasks_in_serve_mode() {
    use chrono::{TimeZone, Utc};
    let h = harness_with(Mode::Serve, true, "http://127.0.0.1:9").await;
    let id = h.seed_market("WinMart", "winmart.vn", 2).await;
    h.call("setup_supermarket_cron_task", json!({"supermarket_id": id, "cron_expression": "30 6 * * *"})).await;
    let mut jobs = mprimcp::scheduler::CronJobs::default();
    let before = Utc.with_ymd_and_hms(2026, 11, 22, 23, 0, 0).unwrap();
    assert!(jobs.tick(&h.app, before).await.is_empty());
    assert_eq!(jobs.next_due(), Some(Utc.with_ymd_and_hms(2026, 11, 22, 23, 30, 0).unwrap()));
    let fired = jobs.tick(&h.app, Utc.with_ymd_and_hms(2026, 11, 22, 23, 30, 5).unwrap()).await;
    assert_eq!(fired, vec![id.clone()]);
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM crawl_tasks WHERE status = 'pending'").await, 2);
    let trigger: String = sqlx::query_scalar("SELECT trigger FROM crawl_runs").fetch_one(&h.app.db).await.unwrap();
    assert_eq!(trigger, "cron");
    assert_eq!(jobs.next_due(), Some(Utc.with_ymd_and_hms(2026, 11, 23, 23, 30, 0).unwrap()));
    let markets = h.call("get_supermarkets", json!({})).await;
    assert!(markets["supermarkets"][0]["schedule"]["next_run"].is_string());
    let status = h.call("get_setup_status", json!({})).await;
    assert_eq!(status["cron_enabled"], true);
    assert_eq!(status["mode"], "serve");
}

#[tokio::test]
async fn ac10_ac13_notification_channel_ok_and_masked() {
    let (api, _srv) = spawn_mock_telegram(true).await;
    let h = harness_with(Mode::Stdio, false, &api).await;
    let token = "123456789:AAbbccddeeffgghhiijjkkllmmnnoopp_xyz";
    let (err, out, text) = call_raw(
        &h.client,
        "set_notification_channel",
        json!({"channel": "telegram", "token": token, "chat_id": "123456789"}),
    )
    .await;
    assert!(!err, "{text}");
    assert_eq!(out["test_ok"], true);
    assert_eq!(out["target_masked"], "123456***xyz");
    assert!(!text.contains(token) && !out.to_string().contains(token));
    let status = h.call("get_setup_status", json!({})).await;
    assert!(!status.to_string().contains(token));
    assert_eq!(status["notification_channels"][0]["last_test_ok"], true);
    let stored: String =
        sqlx::query_scalar("SELECT token FROM notification_channels").fetch_one(&h.app.db).await.unwrap();
    assert_eq!(stored, token);
    let removed = h.call("set_notification_channel", json!({"channel": "telegram", "token": ""})).await;
    assert_eq!(removed["removed"], true);
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM notification_channels").await, 0);
}

#[tokio::test]
async fn ac11_invalid_token_rejected() {
    let h = harness().await;
    let err = h
        .call_err("set_notification_channel", json!({"channel": "telegram", "token": "not-a-token", "chat_id": "1"}))
        .await;
    assert!(err.contains("Telegram bot token"), "{err}");
    let err = h
        .call_err("set_notification_channel", json!({"channel": "discord", "token": "https://example.com/hook"}))
        .await;
    assert!(err.contains("webhook"), "{err}");
    let err = h
        .call_err(
            "set_notification_channel",
            json!({"channel": "telegram", "token": "123456789:AAbbccddeeffgghhiijjkkllmmnnoopp_xyz"}),
        )
        .await;
    assert!(err.contains("chat_id"), "{err}");
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM notification_channels").await, 0);
}

#[tokio::test]
async fn ac12_test_failure_still_saves() {
    let (api, _srv) = spawn_mock_telegram(false).await;
    let h = harness_with(Mode::Stdio, false, &api).await;
    let out = h
        .call(
            "set_notification_channel",
            json!({"channel": "telegram", "token": "123456789:AAbbccddeeffgghhiijjkkllmmnnoopp_xyz", "chat_id": "42"}),
        )
        .await;
    assert_eq!(out["test_ok"], false);
    assert_eq!(out["error"], "Telegram: Unauthorized");
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM notification_channels").await, 1);
}

#[tokio::test]
async fn alerts_trigger_and_summary_reported() {
    let (api, _srv) = spawn_mock_telegram(true).await;
    let h = harness_with(Mode::Stdio, false, &api).await;
    let id = h.seed_market("WinMart", "winmart.vn", 1).await;
    h.call(
        "set_notification_channel",
        json!({"channel": "telegram", "token": "123456789:AAbbccddeeffgghhiijjkkllmmnnoopp_xyz", "chat_id": "42"}),
    )
    .await;
    let alert = h
        .call(
            "set_price_alert",
            json!({"product_name": "san pham 0", "target_price": 70000, "supermarket_ids": ["WinMart"]}),
        )
        .await;
    assert_eq!(alert["created"], true);
    assert_eq!(alert["supermarket_ids"], json!([id]));
    assert_eq!(alert["message"], "Set the alert: Sản phẩm 0 below 70,000 VND.");
    h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
    let task = claim_all(&h).await[0]["task_id"].as_str().unwrap().to_string();
    let out =
        h.call("submit_price_results", json!({"results": [{"task_id": task, "status": "ok", "price": 65000}]})).await;
    assert_eq!(out["alerts_triggered"][0]["target_price"], 70000);
    assert_eq!(out["summary_sent_to"], json!(["telegram"]));
    let alerts = h.call("get_price_alerts", json!({})).await;
    assert_eq!(alerts["alerts"][0]["below_target_now"], true);
    let deleted = h.call("delete_price_alert", json!({"product_name": "san pham"})).await;
    assert_eq!(deleted["deleted"], true);
}

#[tokio::test]
async fn ac19_init_prompt() {
    let h = harness().await;
    let prompts = h.client.list_prompts(None).await.unwrap();
    assert_eq!(prompts.prompts.len(), 1);
    assert_eq!(prompts.prompts[0].name, "init");
    let got = h.client.get_prompt(GetPromptRequestParams::new("init")).await.unwrap();
    let text = serde_json::to_string(&got).unwrap();
    assert!(text.contains("Run MPriMCP Workflow 0 (ONBOARDING)"), "{text}");
}

#[tokio::test]
async fn ac03_tool_list_matches_parity_table() {
    let h = harness().await;
    let tools = h.client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    let expected = [
        "add_product_with_urls",
        "add_supermarket",
        "delete_price_alert",
        "disable_supermarket_cron_task",
        "get_deep_discount_items",
        "get_pending_crawl_tasks",
        "get_price_alerts",
        "get_price_history",
        "get_product_price_comparison",
        "get_search_urls",
        "get_setup_status",
        "get_supermarkets",
        "set_notification_channel",
        "set_preferences",
        "set_price_alert",
        "setup_supermarket_cron_task",
        "submit_price_results",
        "trigger_crawl_now",
        "update_supermarket",
    ];
    assert_eq!(names, expected);
    for t in &tools {
        assert!(t.output_schema.is_some(), "{} has no output schema", t.name);
        assert!(t.annotations.is_some(), "{} has no annotations", t.name);
    }
    let instructions = h.client.peer_info().and_then(|i| i.instructions.clone()).unwrap_or_default();
    assert!(instructions.contains("WORKFLOW 0 - ONBOARDING"));
}

#[tokio::test]
async fn lenient_arguments_like_python_sdk() {
    let h = harness().await;
    let id = h.seed_market("WinMart", "winmart.vn", 0).await;
    let out = h
        .call(
            "add_product_with_urls",
            json!({"name": "Sữa TH 110ml", "urls": format!("[{{\"supermarket_id\": \"{id}\", \"url\": \"https://winmart.vn/p/1\"}}]")}),
        )
        .await;
    assert_eq!(out["urls"][0]["status"], "added");
    let alert = h.call("set_price_alert", json!({"product_name": "sua th", "target_price": "70000"})).await;
    assert_eq!(alert["target_price"], 70000);
}

#[tokio::test]
async fn usd_prices_end_to_end() {
    let h = harness().await;
    let prefs =
        h.call("set_preferences", json!({"currency": "USD", "language": "en", "timezone": "America/New_York"})).await;
    assert_eq!(prefs["currency"], "USD");
    assert_eq!(prefs["currency_locked"], false);
    let status = h.call("get_setup_status", json!({})).await;
    assert_eq!((status["currency"].as_str(), status["language"].as_str()), (Some("USD"), Some("en")));
    assert_eq!(status["timezone"], "America/New_York");

    let id = h.seed_market("Kroger", "kroger.com", 1).await;
    h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
    let tasks = claim_all(&h).await;
    let task = tasks[0]["task_id"].as_str().unwrap().to_string();
    let bad =
        h.call("submit_price_results", json!({"results": [{"task_id": task, "status": "ok", "price": 3.999}]})).await;
    assert!(bad["rejected"][0]["reason"].as_str().unwrap().contains("more than 2 decimals"), "{bad}");
    let out = h
        .call(
            "submit_price_results",
            json!({"results": [{"task_id": task, "status": "ok", "price": 3.99, "original_price": "4.99"}]}),
        )
        .await;
    assert_eq!(out["accepted"], 1);
    assert_eq!(h.sql_i64("SELECT current_price FROM price_records").await, 399);

    let cmp = h.call("get_product_price_comparison", json!({"product_name": "san pham 0"})).await;
    assert_eq!(cmp["currency"], "USD");
    assert_eq!(cmp["prices"][0]["current_price"], 3.99);
    assert_eq!(cmp["prices"][0]["original_price"], 4.99);
    assert_eq!(cmp["prices"][0]["discount_pct"], 20.0);
    let history = h.call("get_price_history", json!({"product_name": "san pham 0"})).await;
    assert_eq!(history["series"][0]["stats"]["latest_price"], 3.99);

    let alert = h.call("set_price_alert", json!({"product_name": "san pham 0", "target_price": 4})).await;
    assert_eq!(alert["target_price"], 4.0);
    assert_eq!(alert["below_target_now"], true);
    assert_eq!(alert["message"], "Set the alert: Sản phẩm 0 below $4.00. It is already below that now.");
    let err = h.call_err("set_price_alert", json!({"product_name": "san pham 0", "target_price": 3.001})).await;
    assert!(err.contains("more than 2 decimals"), "{err}");

    let locked = h.call_err("set_preferences", json!({"currency": "VND"})).await;
    assert!(locked.contains("prices are already stored in USD"), "{locked}");
}

#[tokio::test]
async fn vnd_rejects_decimals_and_keeps_integers() {
    let h = harness().await;
    let id = h.seed_market("WinMart", "winmart.vn", 1).await;
    h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
    let task = claim_all(&h).await[0]["task_id"].as_str().unwrap().to_string();
    let out =
        h.call("submit_price_results", json!({"results": [{"task_id": task, "status": "ok", "price": 45.9}]})).await;
    assert!(out["rejected"][0]["reason"].as_str().unwrap().contains("whole number of VND"), "{out}");
    h.call("submit_price_results", json!({"results": [{"task_id": task, "status": "ok", "price": 45900}]})).await;
    let cmp = h.call("get_product_price_comparison", json!({"product_name": "san pham 0"})).await;
    assert_eq!(cmp["prices"][0]["current_price"].to_string(), "45900");
    assert_eq!(cmp["currency"], "VND");
    let still = h.call("set_preferences", json!({"language": "vi"})).await;
    assert_eq!((still["language"].as_str(), still["currency_locked"].as_bool()), (Some("vi"), Some(true)));
}

#[tokio::test]
async fn timezone_preference_moves_schedules() {
    let h = harness().await;
    let id = h.seed_market("WinMart", "winmart.vn", 0).await;
    h.call("set_preferences", json!({"timezone": "America/New_York"})).await;
    let markets = h.call("get_supermarkets", json!({})).await;
    assert_eq!(markets["timezone"], "America/New_York");
    assert_eq!(markets["supermarkets"][0]["schedule"]["timezone"], "America/New_York");
    let cron =
        h.call("setup_supermarket_cron_task", json!({"supermarket_id": id, "cron_expression": "0 8 * * *"})).await;
    assert_eq!(cron["timezone"], "America/New_York");
    let err = h.call_err("set_preferences", json!({"timezone": "Mars/Olympus"})).await;
    assert!(err.contains("Unknown timezone"), "{err}");
}

#[tokio::test]
async fn currency_env_cannot_contradict_stored_prices() {
    let h = harness().await;
    let id = h.seed_market("WinMart", "winmart.vn", 1).await;
    h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
    let task = claim_all(&h).await[0]["task_id"].as_str().unwrap().to_string();
    h.call("submit_price_results", json!({"results": [{"task_id": task, "status": "ok", "price": 45900}]})).await;
    let mut cfg = config(h.dir.path(), false, "http://127.0.0.1:9");
    cfg.currency = Some(mprimcp::locale::Currency::Usd);
    let err = mprimcp::app::App::open(cfg.clone(), Mode::Stdio).await.err().unwrap().to_string();
    assert!(err.contains("MPRIMCP_CURRENCY=USD") && err.contains("VND"), "{err}");
    cfg.currency = None;
    cfg.language = Some(mprimcp::locale::Lang::Vi);
    let app = mprimcp::app::App::open(cfg.clone(), Mode::Stdio).await.unwrap();
    let c2 = connect(app).await;
    let (err, _, text) = call_raw(&c2, "set_preferences", json!({"language": "en"})).await;
    assert!(err && text.contains("MPRIMCP_LANGUAGE"), "{text}");
}
