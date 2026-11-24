// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};

use crate::clock::human;
use crate::error::{SResult, ServiceError};
use crate::locale::{Currency, Lang, Money};
use crate::mcp::MprServer;
use crate::mcp::types::*;
use crate::prefs::{self, Prefs};
use crate::services::agent_tasks::{self, PriceResult};
use crate::services::alerts::{self, AlertView};
use crate::services::analytics::{self, MarketPrice};
use crate::services::notify::TokenChange;
use crate::services::products::{self, UrlInput};
use crate::services::schedules::{self, CRON_OFF_HINT};
use crate::services::setup_status;
use crate::services::supermarkets::{self, Changes, SEARCH_NOTE};
use crate::text::round1;

fn out<T>(r: SResult<T>) -> Result<Json<T>, String> {
    match r {
        Ok(v) => Ok(Json(v)),
        Err(ServiceError::Internal(e)) => {
            tracing::error!(error = %e, "tool failed");
            Err(format!("Internal error: {e}"))
        }
        Err(e) => Err(e.to_string()),
    }
}

fn money(pr: &Prefs, minor: Option<i64>) -> Option<Money> {
    minor.map(|m| pr.currency.money(m))
}

fn to_minor(currency: Currency, field: &str, value: Option<f64>) -> Result<Option<i64>, String> {
    value.map(|v| currency.to_minor(v).map_err(|e| format!("{field}: {e}"))).transpose()
}

fn price_row(p: &MarketPrice, pr: &Prefs) -> PriceRowOut {
    PriceRowOut {
        supermarket_id: p.supermarket_id.clone(),
        supermarket_name: p.supermarket_name.clone(),
        current_price: money(pr, p.current_price),
        original_price: money(pr, p.original_price),
        member_price: money(pr, p.member_price),
        discount_pct: p.discount_pct,
        member_discount_pct: p.member_discount_pct,
        in_stock: p.in_stock,
        checked_at: human(p.crawled_at, pr.timezone),
        stale: p.is_stale.then_some(true),
        no_data_yet: p.current_price.is_none().then_some(true),
        last_error: p.last_error.clone(),
        url: p.url.clone(),
        source: p.source.clone(),
        needs_review: p.needs_review.then_some(true),
    }
}

fn alert_view(v: &AlertView, pr: &Prefs) -> AlertOut {
    AlertOut {
        product_name: v.product.name.clone(),
        target_price: pr.currency.money(v.alert.target_price),
        supermarket_ids: v.alert.supermarket_ids.clone(),
        below_target_now: v.below(),
        cheapest: v.best.as_ref().map(|b| CheapestOut {
            supermarket_id: b.supermarket_id.clone(),
            supermarket_name: b.supermarket_name.clone(),
            current_price: money(pr, b.current_price),
            checked_at: human(b.crawled_at, pr.timezone),
            needs_review: b.needs_review.then_some(true),
            url: b.url.clone(),
        }),
    }
}

#[tool_router(vis = "pub(crate)")]
impl MprServer {
    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_supermarkets",
        title = "List supermarkets",
        description = "List every configured supermarket with its ID, crawl schedule (cron task) status, next run \
time, number of tracked product URLs, the result of the last crawl, its search URL template, crawl tasks waiting \
for you (pending_tasks) and prices flagged for review (needs_review_count).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_supermarkets(&self) -> Result<Json<SupermarketsOut>, String> {
        out(async {
            agent_tasks::maintain(&self.app.db).await?;
            let pr = prefs::load(&self.app.db).await?;
            let server_cron = self.app.server_cron();
            let summaries = supermarkets::list_summaries(&self.app.db).await?;
            let items = summaries
                .into_iter()
                .map(|item| {
                    let m = &item.supermarket;
                    SupermarketOut {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        base_url: m.base_url.clone(),
                        schedule: ScheduleOut {
                            cron_expression: m.cron_expression.clone(),
                            enabled: m.cron_enabled,
                            timezone: m.timezone.clone(),
                            next_run: human(
                                schedules::next_run(m, server_cron).map(|t| t.timestamp_millis()),
                                pr.timezone,
                            ),
                            crawl_in_progress: false,
                        },
                        tracked_urls: item.active_url_count,
                        failing_urls: item.failing_url_count,
                        search_url_template: m.search_url_template.clone(),
                        pending_tasks: item.pending_tasks,
                        needs_review_count: item.needs_review_count,
                        last_run: item.last_run.as_ref().map(|r| LastRunOut {
                            status: r.status.clone(),
                            trigger: r.trigger.clone(),
                            started_at: human(Some(r.started_at), pr.timezone),
                            urls_ok: r.urls_ok,
                            urls_failed: r.urls_failed,
                            error: r.error.clone(),
                        }),
                    }
                })
                .collect();
            Ok(SupermarketsOut { supermarkets: items, timezone: pr.tz_name().into() })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "add_supermarket",
        title = "Add supermarket",
        description = "Add a supermarket to track. The server creates its ID from the name (do not pass one) and \
returns it as supermarket_id; the other tools accept that ID or the supermarket's name. After adding, set a crawl \
schedule with setup_supermarket_cron_task.",
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn add_supermarket(
        &self,
        Parameters(p): Parameters<AddSupermarketParams>,
    ) -> Result<Json<AddSupermarketOut>, String> {
        out(async {
            let m = supermarkets::add(&self.app.db, &p.name, &p.base_url, p.search_url_template.as_deref()).await?;
            Ok(AddSupermarketOut {
                message: format!("Added {} with ID '{}'. It has no crawl schedule yet.", m.name, m.id),
                id: m.id.clone(),
                supermarket_id: m.id,
                name: m.name,
                base_url: m.base_url,
                search_url_template: m.search_url_template,
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "update_supermarket",
        title = "Update supermarket",
        description = "Change a supermarket's name, website or search URL template. Only the fields you pass are \
changed; takes effect immediately. The crawl schedule is not touched.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn update_supermarket(
        &self,
        Parameters(p): Parameters<UpdateSupermarketParams>,
    ) -> Result<Json<UpdateSupermarketOut>, String> {
        out(async {
            let mut updated: Vec<String> = Vec::new();
            if p.name.is_some() {
                updated.push("name".into());
            }
            if p.base_url.is_some() {
                updated.push("base_url".into());
            }
            if p.search_url_template.is_some() {
                updated.push("search_url_template".into());
            }
            if updated.is_empty() {
                return Err(ServiceError::Validation(
                    "Nothing to update: pass name, base_url or search_url_template.".into(),
                ));
            }
            updated.sort();
            let changes = Changes {
                name: p.name,
                base_url: p.base_url,
                search_url_template: p.search_url_template,
                timezone: None,
            };
            let m = supermarkets::update(&self.app.db, &p.supermarket_id, changes).await?;
            Ok(UpdateSupermarketOut {
                message: format!("Updated {}: {}.", m.name, updated.join(", ")),
                id: m.id,
                name: m.name,
                base_url: m.base_url,
                search_url_template: m.search_url_template,
                updated,
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_search_urls",
        title = "Search URLs",
        description = "Search-results page URL for the keyword at each supermarket, built from its \
search_url_template (keyword URL-encoded). Open these with your browser tool to find product pages. search_url is \
null when the supermarket has no template: then open base_url and use the site's search box. This tool does not \
access the internet.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_search_urls(
        &self,
        Parameters(p): Parameters<SearchUrlsParams>,
    ) -> Result<Json<SearchUrlsOut>, String> {
        out(async {
            let found = supermarkets::search_urls(&self.app.db, &p.keyword, p.supermarket_ids.as_deref()).await?;
            Ok(SearchUrlsOut {
                keyword: crate::text::collapse_spaces(&p.keyword),
                supermarkets: found
                    .into_iter()
                    .map(|r| SearchUrlOut {
                        note: r.url.is_none().then(|| SEARCH_NOTE.to_string()),
                        supermarket_id: r.supermarket.id,
                        name: r.supermarket.name,
                        base_url: r.supermarket.base_url,
                        search_url: r.url,
                    })
                    .collect(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "setup_supermarket_cron_task",
        title = "Set crawl schedule",
        description = "Create or replace the independent crawl schedule (cron task) for one supermarket. Takes \
effect immediately, no restart. Each supermarket has its own job, so a slow or failing site never delays the others. \
Also re-enables a disabled schedule. The schedule is always saved; cron_enabled=false means this server does not run \
schedules (see hint).",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn setup_supermarket_cron_task(
        &self,
        Parameters(p): Parameters<CronParams>,
    ) -> Result<Json<CronOut>, String> {
        out(async {
            let r = schedules::set_cron(&self.app.db, &p.supermarket_id, &p.cron_expression).await?;
            let pr = prefs::load(&self.app.db).await?;
            let cron_enabled = self.app.server_cron();
            Ok(CronOut {
                message: format!("Crawl schedule for {} set to '{}'.", r.supermarket.name, r.cron_expression),
                supermarket_id: r.supermarket.id,
                supermarket_name: r.supermarket.name,
                cron_expression: r.cron_expression,
                timezone: r.timezone,
                next_runs: r.next_runs.iter().filter_map(|t| human(Some(t.timestamp_millis()), pr.timezone)).collect(),
                cron_enabled,
                hint: (!cron_enabled).then(|| CRON_OFF_HINT.to_string()),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "disable_supermarket_cron_task",
        title = "Pause crawl schedule",
        description = "Pause a supermarket's crawl schedule. The cron expression is kept; call \
setup_supermarket_cron_task again to resume.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn disable_supermarket_cron_task(
        &self,
        Parameters(p): Parameters<SupermarketParams>,
    ) -> Result<Json<DisableCronOut>, String> {
        out(async {
            let m = schedules::disable_cron(&self.app.db, &p.supermarket_id).await?;
            Ok(DisableCronOut {
                message: format!("Paused the crawl schedule for {}.", m.name),
                supermarket_id: m.id,
                cron_expression: m.cron_expression,
                enabled: false,
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "trigger_crawl_now",
        title = "Crawl now",
        description = "Queue a crawl task for every tracked product page of one supermarket, right now. The server \
does not fetch pages itself: you read the prices with your browser tool (get_pending_crawl_tasks, then \
submit_price_results). Pages that already have an open task are not queued twice.",
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn trigger_crawl_now(
        &self,
        Parameters(p): Parameters<SupermarketParams>,
    ) -> Result<Json<TriggerOut>, String> {
        out(async {
            let r = schedules::trigger_now(&self.app.db, &p.supermarket_id).await?;
            let message = if r.queued > 0 {
                format!(
                    "Queued {} crawl task(s) for {}. Read them with get_pending_crawl_tasks and send the prices \
                     with submit_price_results.",
                    r.queued, r.supermarket.name
                )
            } else {
                format!(
                    "Every tracked page of {} already has an open crawl task; nothing new was queued.",
                    r.supermarket.name
                )
            };
            Ok(TriggerOut {
                supermarket_id: r.supermarket.id,
                started: r.queued > 0,
                queued_tasks: r.queued,
                already_open: r.already_open,
                pending_tasks: r.open_tasks,
                message,
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "add_product_with_urls",
        title = "Track product",
        description = "Start tracking a product's price at one or more supermarkets. If a product with the same name \
(ignoring accents/case) already exists, the URLs are added to it or update its existing URL for that supermarket. \
Prices are collected on each supermarket's crawl schedule (or use trigger_crawl_now).",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn add_product_with_urls(
        &self,
        Parameters(p): Parameters<AddProductParams>,
    ) -> Result<Json<AddProductOut>, String> {
        out(async {
            let inputs: Vec<UrlInput> = p
                .urls
                .iter()
                .map(|u| UrlInput { supermarket_id: u.supermarket_id.clone(), url: u.url.clone() })
                .collect();
            let r = products::add_product_with_urls(&self.app.db, &p.name, p.category.as_deref(), &inputs).await?;
            let ok = r.urls.iter().filter(|u| u.status != "error").count();
            Ok(AddProductOut {
                message: format!(
                    "{} '{}' at {ok} supermarket(s).",
                    if r.created { "Now tracking" } else { "Updated" },
                    r.product.name
                ),
                product_id: r.product.id,
                name: r.product.name,
                category: r.product.category,
                created: r.created,
                urls: r
                    .urls
                    .into_iter()
                    .map(|u| UrlResultOut {
                        supermarket_id: u.supermarket_id,
                        url: u.url,
                        status: u.status.into(),
                        error: u.message,
                        warning: u.warning,
                    })
                    .collect(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_product_price_comparison",
        title = "Compare prices",
        description = "Latest price of a tracked product at every supermarket, cheapest first, with original \
(pre-discount) and member prices and discount percentages. If the name matches several products closely, \
other_matches lists them. A price with needs_review=true is unverified.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_product_price_comparison(
        &self,
        Parameters(p): Parameters<ProductNameParams>,
    ) -> Result<Json<ComparisonOut>, String> {
        out(async {
            let cmp = analytics::price_comparison(&self.app.db, &p.product_name).await?;
            let pr = prefs::load(&self.app.db).await?;
            Ok(ComparisonOut {
                product: ProductRef { id: cmp.product.id, name: cmp.product.name, category: cmp.product.category },
                currency: pr.currency.code().into(),
                prices: cmp.prices.iter().map(|x| price_row(x, &pr)).collect(),
                cheapest: cmp.cheapest.as_ref().map(|x| price_row(x, &pr)),
                other_matches: cmp.alternatives.into_iter().map(|m| m.name).collect(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_deep_discount_items",
        title = "Deep discounts",
        description = "Products whose latest price is discounted by at least the given percentage, deepest discount \
first. Prices flagged needs_review are left out.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_deep_discount_items(
        &self,
        Parameters(p): Parameters<DeepDiscountParams>,
    ) -> Result<Json<DiscountsOut>, String> {
        out(async {
            let items =
                analytics::deep_discounts(&self.app.db, p.min_discount_percentage as f64, p.days, p.limit).await?;
            let pr = prefs::load(&self.app.db).await?;
            Ok(DiscountsOut {
                currency: pr.currency.code().into(),
                count: items.len(),
                items: items
                    .into_iter()
                    .map(|i| DiscountOut {
                        product_name: i.product_name,
                        category: i.category,
                        supermarket_id: i.supermarket_id,
                        supermarket_name: i.supermarket_name,
                        current_price: pr.currency.money(i.current_price),
                        original_price: money(&pr, i.original_price),
                        discount_pct: i.discount_pct,
                        member_price: money(&pr, i.member_price),
                        member_discount_pct: i.member_discount_pct,
                        checked_at: human(Some(i.crawled_at), pr.timezone),
                        url: i.url,
                    })
                    .collect(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_price_history",
        title = "Price history",
        description = "Daily price history of a product per supermarket (last price of each day) with min / max / \
average and how today's price compares with the period's lowest.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_price_history(&self, Parameters(p): Parameters<HistoryParams>) -> Result<Json<HistoryOut>, String> {
        out(async {
            let h =
                analytics::price_history(&self.app.db, &p.product_name, p.days, p.supermarket_id.as_deref()).await?;
            let pr = prefs::load(&self.app.db).await?;
            let series = h
                .series
                .iter()
                .map(|s| {
                    let low = s.min_point();
                    let latest = s.latest();
                    SeriesOut {
                        supermarket_id: s.supermarket_id.clone(),
                        supermarket_name: s.supermarket_name.clone(),
                        points: s
                            .points
                            .iter()
                            .map(|pt| HistoryPointOut {
                                date: pt.day.format("%Y-%m-%d").to_string(),
                                current_price: pr.currency.money(pt.current_price),
                                original_price: money(&pr, pt.original_price),
                                member_price: money(&pr, pt.member_price),
                                needs_review: pt.needs_review.then_some(true),
                            })
                            .collect(),
                        stats: HistoryStatsOut {
                            min_price: money(&pr, low.map(|l| l.current_price)),
                            min_date: low.map(|l| l.day.format("%Y-%m-%d").to_string()),
                            max_price: money(&pr, s.max_price()),
                            avg_price: money(&pr, s.avg_price()),
                            latest_price: money(&pr, latest.map(|l| l.current_price)),
                            latest_vs_min_pct: match (latest, low) {
                                (Some(l), Some(m)) => Some(round1(
                                    (l.current_price - m.current_price) as f64 / m.current_price as f64 * 100.0,
                                )),
                                _ => None,
                            },
                        },
                    }
                })
                .collect();
            Ok(HistoryOut {
                product: HistoryProductRef { id: h.product.id, name: h.product.name },
                days: h.days,
                currency: pr.currency.code().into(),
                series,
                other_matches: h.alternatives.into_iter().map(|m| m.name).collect(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_pending_crawl_tasks",
        title = "Get crawl tasks",
        description = "Take the oldest pending crawl tasks and reserve them for you until lease_expires_at. For each \
task, open its url with your browser tool, read the price of that product only, and send the result with \
submit_price_results BEFORE lease_expires_at; after that the task goes back to the queue. remaining_pending is how \
many tasks are still waiting after these.",
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn get_pending_crawl_tasks(&self, Parameters(p): Parameters<ClaimParams>) -> Result<Json<TasksOut>, String> {
        out(async {
            let r = agent_tasks::claim_tasks(&self.app.db, p.supermarket_id.as_deref(), p.limit).await?;
            let pr = prefs::load(&self.app.db).await?;
            let tasks: Vec<TaskOut> = r
                .tasks
                .into_iter()
                .map(|t| TaskOut {
                    task_id: t.task.id,
                    product_name: t.product_name,
                    category: t.category,
                    supermarket_id: t.task.supermarket_id,
                    supermarket_name: t.supermarket_name,
                    url: t.task.url,
                    last_price: money(&pr, t.last_price),
                    last_original_price: money(&pr, t.last_original_price),
                    last_checked_at: human(t.last_checked_at, pr.timezone),
                    lease_expires_at: human(t.task.lease_expires_at, pr.timezone),
                })
                .collect();
            Ok(TasksOut {
                count: tasks.len(),
                tasks,
                remaining_pending: r.remaining_pending,
                currency: pr.currency.code().into(),
                timezone: pr.tz_name().into(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "submit_price_results",
        title = "Submit prices",
        description = "Send the prices you read for tasks from get_pending_crawl_tasks. Each result is checked on its \
own: invalid ones are listed in rejected with the reason (fix and send them again); the rest are saved. A price more \
than 50% away from the last one is saved but flagged needs_review. Never guess a price. The server itself messages \
the user about alerts_triggered and, once nothing is pending, a summary of where each product is cheapest \
(summary_sent_to): do not send those again.",
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn submit_price_results(&self, Parameters(p): Parameters<SubmitParams>) -> Result<Json<SubmitOut>, String> {
        out(async {
            let pr = prefs::load(&self.app.db).await?;
            let c = pr.currency;
            let inputs: Vec<PriceResult> = p
                .results
                .iter()
                .map(|i| {
                    let amounts = (|| -> Result<_, String> {
                        Ok((
                            to_minor(c, "price", i.price)?,
                            to_minor(c, "original_price", i.original_price)?,
                            to_minor(c, "member_price", i.member_price)?,
                        ))
                    })();
                    let (price, original_price, member_price, amount_error) = match amounts {
                        Ok((a, b, m)) => (a, b, m, None),
                        Err(e) => (None, None, None, Some(e)),
                    };
                    PriceResult {
                        task_id: i.task_id.clone(),
                        status: i.status.as_str().into(),
                        price,
                        original_price,
                        member_price,
                        raw_text: i.raw_text.clone(),
                        note: i.note.clone(),
                        amount_error,
                    }
                })
                .collect();
            let r = agent_tasks::submit_results(&self.app.db, &inputs).await?;
            let sent = self.app.notifier.after_submit(&r.saved, r.queue_empty, r.accepted).await;
            let notifications = ((!sent.alerts.is_empty() || (r.queue_empty && r.accepted > 0))
                && sent.channels.is_empty())
            .then(|| {
                "No Telegram/Discord channel is set up (set_notification_channel or the dashboard), so the user was \
                 not messaged: tell them in your summary."
                    .to_string()
            });
            let alerts_triggered: Vec<AlertTriggeredOut> = sent
                .alerts
                .iter()
                .map(|h| AlertTriggeredOut {
                    product_name: h.product_name.clone(),
                    target_price: c.money(h.target_price),
                    prices: h
                        .prices
                        .iter()
                        .map(|x| AlertPriceOut { supermarket_id: x.supermarket_id.clone(), price: c.money(x.price) })
                        .collect(),
                })
                .collect();
            Ok(SubmitOut {
                accepted: r.accepted,
                alerts_triggered: (!alerts_triggered.is_empty()).then_some(alerts_triggered),
                summary_sent_to: (!sent.summary_sent_to.is_empty()).then_some(sent.summary_sent_to),
                notifications,
                requeued: (r.requeued > 0).then_some(r.requeued),
                flagged: r
                    .flagged
                    .into_iter()
                    .map(|f| FlaggedOut {
                        task_id: f.task_id,
                        product_name: f.product_name,
                        supermarket_id: f.supermarket_id,
                        price: c.money(f.price),
                        last_price: c.money(f.last_price),
                        change_pct: f.change_pct,
                    })
                    .collect(),
                rejected: r
                    .rejected
                    .into_iter()
                    .map(|x| RejectedOut { task_id: x.task_id, reason: x.reason })
                    .collect(),
                warnings: (!r.warnings.is_empty()).then_some(r.warnings),
                remaining_pending: r.remaining_pending,
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "set_price_alert",
        title = "Set price alert",
        description = "Tell the user (Telegram / Discord, set with set_notification_channel or on the dashboard) as \
soon as a crawl saves a price of this product below target_price. One alert per product: calling again replaces it. \
Check product_name in the answer: it is the matched product.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn set_price_alert(&self, Parameters(p): Parameters<SetAlertParams>) -> Result<Json<SetAlertOut>, String> {
        out(async {
            let pr = prefs::load(&self.app.db).await?;
            let target = pr
                .currency
                .to_minor(p.target_price)
                .map_err(|e| ServiceError::Validation(format!("target_price: {e}")))?;
            let (v, created) =
                alerts::set_alert(&self.app.db, &p.product_name, target, p.supermarket_ids.as_deref()).await?;
            let mut message = format!(
                "{} the alert: {} below {}.",
                if created { "Set" } else { "Updated" },
                v.product.name,
                pr.currency.format_code(target)
            );
            if v.below() {
                message.push_str(" It is already below that now.");
            }
            Ok(SetAlertOut { alert: alert_view(&v, &pr), created, message })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_price_alerts",
        title = "List price alerts",
        description = "Every price alert with its target and the cheapest current price it covers.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_price_alerts(&self) -> Result<Json<AlertsOut>, String> {
        out(async {
            let found = alerts::list_alerts(&self.app.db).await?;
            let pr = prefs::load(&self.app.db).await?;
            Ok(AlertsOut {
                count: found.len(),
                alerts: found.iter().map(|v| alert_view(v, &pr)).collect(),
                currency: pr.currency.code().into(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "delete_price_alert",
        title = "Delete price alert",
        description = "Remove the price alert of a product (the product stays tracked).",
        annotations(read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn delete_price_alert(
        &self,
        Parameters(p): Parameters<DeleteAlertParams>,
    ) -> Result<Json<DeleteAlertOut>, String> {
        out(async {
            let name = alerts::delete_alert(&self.app.db, &p.product_name).await?;
            Ok(DeleteAlertOut { message: format!("Removed the alert for {name}."), product_name: name, deleted: true })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "get_setup_status",
        title = "Setup status",
        description = "What is already set up (supermarkets, products, price alerts, notification channels, schedules, \
prices), the server's currency, language and timezone, and the suggested next onboarding step (next_step). Call it \
first in Workflow 0 (ONBOARDING); optional steps (set_alerts, notifications) never block the later ones.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_setup_status(&self) -> Result<Json<SetupStatusOut>, String> {
        out(async {
            let s = setup_status::status(&self.app.db).await?;
            let pr = prefs::load(&self.app.db).await?;
            Ok(SetupStatusOut {
                supermarkets: SetupMarketsOut {
                    count: s.supermarkets.len(),
                    items: s
                        .supermarkets
                        .into_iter()
                        .map(|m| SetupMarketOut {
                            supermarket_id: m.supermarket_id,
                            name: m.name,
                            has_search_url_template: m.has_search_url_template,
                            has_schedule: m.has_schedule,
                        })
                        .collect(),
                },
                products: s.products,
                products_without_urls: s.products_without_urls,
                price_alerts: s.price_alerts,
                notification_channels: s
                    .notification_channels
                    .into_iter()
                    .map(|c| SetupChannelOut {
                        channel: c.channel,
                        enabled: c.enabled,
                        target_masked: c.target_masked,
                        last_test_ok: c.last_test_ok,
                        last_test_at: human(c.last_test_at, pr.timezone),
                    })
                    .collect(),
                cron_enabled: self.app.server_cron(),
                mode: self.app.mode.as_str().into(),
                has_any_price: s.has_any_price,
                currency: pr.currency.code().into(),
                language: pr.language.code().into(),
                timezone: pr.tz_name().into(),
                next_step: s.next_step.into(),
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "set_notification_channel",
        title = "Set notification channel",
        description = "Save the user's Telegram bot (token + chat_id) or Discord webhook for price alerts and the \
post-crawl summary, then send a test message right away. The answer never contains the full token: show the user \
target_masked. If test_ok is false the setting is still saved; tell the user the error. An empty token removes the \
channel.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = true)
    )]
    async fn set_notification_channel(
        &self,
        Parameters(p): Parameters<SetChannelParams>,
    ) -> Result<Json<SetChannelOut>, String> {
        out(async {
            let r = self
                .app
                .notifier
                .set_channel(
                    p.channel.as_str(),
                    TokenChange::Set(p.token),
                    p.chat_id.as_deref(),
                    p.enabled.unwrap_or(true),
                )
                .await?;
            let label = if r.channel == "telegram" { "Telegram" } else { "Discord" };
            let message = if r.removed {
                format!("Removed the {label} channel.")
            } else if r.test_ok {
                format!("Saved {label} and sent a test message.")
            } else {
                format!("Saved {label}, but the test message failed: {}", r.error.clone().unwrap_or_default())
            };
            Ok(SetChannelOut {
                channel: r.channel,
                target_masked: r.target_masked,
                enabled: r.enabled,
                test_ok: r.test_ok,
                error: r.error,
                removed: r.removed,
                message,
            })
        }
        .await)
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    #[tool(
        name = "set_preferences",
        title = "Set preferences",
        description = "Set the server's currency (VND or USD), the language of the dashboard and of the Telegram/Discord \
messages (en or vi) and the timezone used for schedules and displayed times (IANA name). Only the fields you pass \
change. The currency can only change while no price is stored. Settings fixed by the server's environment cannot be \
changed here.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn set_preferences(
        &self,
        Parameters(p): Parameters<SetPreferencesParams>,
    ) -> Result<Json<PreferencesOut>, String> {
        out(async {
            let changes = prefs::Changes {
                currency: p.currency.map(|c| match c {
                    CurrencyCode::VND => Currency::Vnd,
                    CurrencyCode::USD => Currency::Usd,
                }),
                language: p.language.map(|l| match l {
                    LanguageCode::En => Lang::En,
                    LanguageCode::Vi => Lang::Vi,
                }),
                timezone: p.timezone,
            };
            let pr = prefs::update(&self.app.db, &self.app.config, changes).await?;
            let locked = analytics::has_any_price(&self.app.db).await? || self.app.config.currency.is_some();
            Ok(PreferencesOut {
                message: format!(
                    "Currency {}, language {}, timezone {}.",
                    pr.currency.code(),
                    pr.language.code(),
                    pr.tz_name()
                ),
                currency: pr.currency.code().into(),
                language: pr.language.code().into(),
                timezone: pr.tz_name().into(),
                currency_locked: locked,
            })
        }
        .await)
    }
}
