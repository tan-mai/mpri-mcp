-- TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
-- Times are Unix epoch milliseconds (UTC). Booleans are 0/1.

CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    session_version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_login_at INTEGER,
    password_changed_at INTEGER NOT NULL
);

CREATE TABLE app_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE supermarkets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    name_key TEXT NOT NULL UNIQUE,
    base_url TEXT NOT NULL,
    search_url_template TEXT,
    cron_expression TEXT,
    cron_enabled INTEGER NOT NULL DEFAULT 0 CHECK (cron_enabled IN (0, 1)),
    timezone TEXT NOT NULL DEFAULT 'Asia/Ho_Chi_Minh',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE products (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    name_key TEXT NOT NULL UNIQUE,
    category TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE product_urls (
    id INTEGER PRIMARY KEY,
    product_id INTEGER NOT NULL REFERENCES products (id) ON DELETE CASCADE,
    supermarket_id TEXT NOT NULL REFERENCES supermarkets (id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0, 1)),
    in_stock INTEGER CHECK (in_stock IN (0, 1)),
    last_crawled_at INTEGER,
    last_success_at INTEGER,
    last_error TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE (product_id, supermarket_id)
);
CREATE INDEX ix_product_urls_supermarket ON product_urls (supermarket_id);

CREATE TABLE price_records (
    id INTEGER PRIMARY KEY,
    product_url_id INTEGER NOT NULL REFERENCES product_urls (id) ON DELETE CASCADE,
    current_price INTEGER NOT NULL,
    original_price INTEGER,
    member_price INTEGER,
    in_stock INTEGER CHECK (in_stock IN (0, 1)),
    source TEXT NOT NULL DEFAULT 'agent',
    needs_review INTEGER NOT NULL DEFAULT 0 CHECK (needs_review IN (0, 1)),
    raw_text TEXT,
    crawled_at INTEGER NOT NULL
);
CREATE INDEX ix_price_records_url_time ON price_records (product_url_id, crawled_at);
CREATE INDEX ix_price_records_review ON price_records (needs_review) WHERE needs_review = 1;

CREATE TABLE crawl_runs (
    id INTEGER PRIMARY KEY,
    supermarket_id TEXT NOT NULL REFERENCES supermarkets (id) ON DELETE CASCADE,
    trigger TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    urls_total INTEGER NOT NULL DEFAULT 0,
    urls_ok INTEGER NOT NULL DEFAULT 0,
    urls_failed INTEGER NOT NULL DEFAULT 0,
    error TEXT
);
CREATE INDEX ix_crawl_runs_supermarket ON crawl_runs (supermarket_id, id);

CREATE TABLE crawl_tasks (
    id TEXT PRIMARY KEY,
    product_url_id INTEGER NOT NULL REFERENCES product_urls (id) ON DELETE CASCADE,
    supermarket_id TEXT NOT NULL REFERENCES supermarkets (id) ON DELETE CASCADE,
    run_id INTEGER REFERENCES crawl_runs (id) ON DELETE SET NULL,
    url TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'claimed', 'done', 'failed', 'expired')),
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    claimed_at INTEGER,
    lease_expires_at INTEGER,
    completed_at INTEGER,
    last_error TEXT
);
CREATE INDEX ix_crawl_tasks_status_created ON crawl_tasks (status, created_at);
CREATE INDEX ix_crawl_tasks_url ON crawl_tasks (product_url_id);
CREATE INDEX ix_crawl_tasks_run ON crawl_tasks (run_id);

CREATE TABLE price_alerts (
    id INTEGER PRIMARY KEY,
    product_id INTEGER NOT NULL UNIQUE REFERENCES products (id) ON DELETE CASCADE,
    target_price INTEGER NOT NULL,
    all_supermarkets INTEGER NOT NULL DEFAULT 1 CHECK (all_supermarkets IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE price_alert_supermarkets (
    alert_id INTEGER NOT NULL REFERENCES price_alerts (id) ON DELETE CASCADE,
    supermarket_id TEXT NOT NULL REFERENCES supermarkets (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (alert_id, supermarket_id)
);

CREATE TABLE price_alert_notified (
    alert_id INTEGER NOT NULL REFERENCES price_alerts (id) ON DELETE CASCADE,
    supermarket_id TEXT NOT NULL REFERENCES supermarkets (id) ON DELETE CASCADE,
    price INTEGER NOT NULL,
    PRIMARY KEY (alert_id, supermarket_id)
);

CREATE TABLE notification_channels (
    channel TEXT PRIMARY KEY CHECK (channel IN ('telegram', 'discord')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    token TEXT NOT NULL,
    chat_id TEXT,
    last_test_ok INTEGER CHECK (last_test_ok IN (0, 1)),
    last_test_at INTEGER,
    last_test_error TEXT,
    updated_at INTEGER NOT NULL
);

CREATE TABLE notification_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    summary_enabled INTEGER NOT NULL DEFAULT 1 CHECK (summary_enabled IN (0, 1)),
    last_summary_at INTEGER
);

CREATE TABLE oauth_clients (
    client_id TEXT PRIMARY KEY,
    client_secret TEXT,
    client_name TEXT,
    metadata TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
);

CREATE TABLE oauth_pending (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    redirect_uri_explicit INTEGER NOT NULL CHECK (redirect_uri_explicit IN (0, 1)),
    state TEXT,
    code_challenge TEXT NOT NULL,
    scopes TEXT NOT NULL,
    resource TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

CREATE TABLE oauth_codes (
    code_hash TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    redirect_uri_explicit INTEGER NOT NULL CHECK (redirect_uri_explicit IN (0, 1)),
    code_challenge TEXT NOT NULL,
    scopes TEXT NOT NULL,
    resource TEXT,
    subject TEXT,
    family_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER
);

CREATE TABLE oauth_tokens (
    token_hash TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('access', 'refresh')),
    client_id TEXT NOT NULL REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    scopes TEXT NOT NULL,
    resource TEXT,
    subject TEXT,
    family_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER,
    revoked_at INTEGER,
    last_used_at INTEGER
);
CREATE INDEX ix_oauth_tokens_client ON oauth_tokens (client_id);
CREATE INDEX ix_oauth_tokens_family ON oauth_tokens (family_id);
