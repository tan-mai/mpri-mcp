CREATE TABLE admin_users (
	id INTEGER NOT NULL, 
	username VARCHAR(100) NOT NULL, 
	password_hash VARCHAR(300) NOT NULL, 
	session_version INTEGER NOT NULL, 
	created_at DATETIME NOT NULL, 
	last_login_at DATETIME, 
	password_changed_at DATETIME NOT NULL, 
	PRIMARY KEY (id), 
	UNIQUE (username)
);

CREATE TABLE app_settings (
	"key" VARCHAR(100) NOT NULL, 
	value JSON NOT NULL, 
	updated_at DATETIME NOT NULL, 
	PRIMARY KEY ("key")
);

CREATE TABLE notification_log (
	id INTEGER NOT NULL, 
	kind VARCHAR(20) NOT NULL, 
	channel VARCHAR(20) NOT NULL, 
	status VARCHAR(20) NOT NULL, 
	message TEXT NOT NULL, 
	error TEXT, 
	created_at DATETIME NOT NULL, 
	PRIMARY KEY (id)
);

CREATE INDEX ix_notification_log_created_at ON notification_log (created_at);

CREATE TABLE oauth_clients (
	client_id VARCHAR(100) NOT NULL, 
	client_secret VARCHAR(200), 
	client_name VARCHAR(200), 
	info JSON NOT NULL, 
	created_at DATETIME NOT NULL, 
	last_used_at DATETIME, 
	revoked_at DATETIME, 
	PRIMARY KEY (client_id)
);

CREATE TABLE products (
	id INTEGER NOT NULL, 
	name VARCHAR(300) NOT NULL, 
	normalized_name VARCHAR(300) NOT NULL, 
	category VARCHAR(100), 
	created_at DATETIME NOT NULL, 
	PRIMARY KEY (id)
);

CREATE UNIQUE INDEX ix_products_normalized_name ON products (normalized_name);

CREATE TABLE supermarkets (
	id VARCHAR(64) NOT NULL, 
	name VARCHAR(200) NOT NULL, 
	base_url VARCHAR(500) NOT NULL, 
	cron_expression VARCHAR(100), 
	cron_enabled BOOLEAN NOT NULL, 
	timezone VARCHAR(64) NOT NULL, 
	search_url_template VARCHAR(500), 
	created_at DATETIME NOT NULL, 
	updated_at DATETIME NOT NULL, 
	PRIMARY KEY (id), 
	UNIQUE (name)
);

CREATE TABLE crawl_runs (
	id INTEGER NOT NULL, 
	supermarket_id VARCHAR(64) NOT NULL, 
	"trigger" VARCHAR(20) NOT NULL, 
	status VARCHAR(20) NOT NULL, 
	started_at DATETIME NOT NULL, 
	finished_at DATETIME, 
	urls_total INTEGER NOT NULL, 
	urls_ok INTEGER NOT NULL, 
	urls_failed INTEGER NOT NULL, 
	error TEXT, 
	PRIMARY KEY (id), 
	FOREIGN KEY(supermarket_id) REFERENCES supermarkets (id) ON DELETE CASCADE
);

CREATE INDEX ix_crawl_runs_started_at ON crawl_runs (started_at);

CREATE INDEX ix_crawl_runs_supermarket_id ON crawl_runs (supermarket_id);

CREATE TABLE oauth_authorization_codes (
	code_hash VARCHAR(64) NOT NULL, 
	client_id VARCHAR(100) NOT NULL, 
	redirect_uri VARCHAR(1000) NOT NULL, 
	redirect_uri_provided_explicitly BOOLEAN NOT NULL, 
	code_challenge VARCHAR(200) NOT NULL, 
	scopes JSON NOT NULL, 
	resource VARCHAR(500), 
	subject VARCHAR(100), 
	family_id VARCHAR(64) NOT NULL, 
	created_at DATETIME NOT NULL, 
	expires_at DATETIME NOT NULL, 
	used_at DATETIME, 
	PRIMARY KEY (code_hash), 
	FOREIGN KEY(client_id) REFERENCES oauth_clients (client_id) ON DELETE CASCADE
);

CREATE TABLE oauth_pending_authorizations (
	id VARCHAR(64) NOT NULL, 
	client_id VARCHAR(100) NOT NULL, 
	redirect_uri VARCHAR(1000) NOT NULL, 
	redirect_uri_provided_explicitly BOOLEAN NOT NULL, 
	state VARCHAR(500), 
	code_challenge VARCHAR(200) NOT NULL, 
	scopes JSON NOT NULL, 
	resource VARCHAR(500), 
	created_at DATETIME NOT NULL, 
	expires_at DATETIME NOT NULL, 
	PRIMARY KEY (id), 
	FOREIGN KEY(client_id) REFERENCES oauth_clients (client_id) ON DELETE CASCADE
);

CREATE TABLE oauth_tokens (
	token_hash VARCHAR(64) NOT NULL, 
	kind VARCHAR(10) NOT NULL, 
	client_id VARCHAR(100) NOT NULL, 
	scopes JSON NOT NULL, 
	resource VARCHAR(500), 
	subject VARCHAR(100), 
	family_id VARCHAR(64) NOT NULL, 
	created_at DATETIME NOT NULL, 
	expires_at DATETIME NOT NULL, 
	used_at DATETIME, 
	revoked_at DATETIME, 
	last_used_at DATETIME, 
	PRIMARY KEY (token_hash), 
	FOREIGN KEY(client_id) REFERENCES oauth_clients (client_id) ON DELETE CASCADE
);

CREATE INDEX ix_oauth_tokens_family_id ON oauth_tokens (family_id);

CREATE INDEX ix_oauth_tokens_client_id ON oauth_tokens (client_id);

CREATE TABLE price_alerts (
	id INTEGER NOT NULL, 
	product_id INTEGER NOT NULL, 
	target_price INTEGER NOT NULL, 
	supermarket_ids JSON, 
	notified JSON NOT NULL, 
	created_at DATETIME NOT NULL, 
	updated_at DATETIME NOT NULL, 
	PRIMARY KEY (id), 
	UNIQUE (product_id), 
	FOREIGN KEY(product_id) REFERENCES products (id) ON DELETE CASCADE
);

CREATE TABLE product_urls (
	id INTEGER NOT NULL, 
	product_id INTEGER NOT NULL, 
	supermarket_id VARCHAR(64) NOT NULL, 
	url VARCHAR(1000) NOT NULL, 
	is_active BOOLEAN NOT NULL, 
	last_crawled_at DATETIME, 
	last_success_at DATETIME, 
	last_error TEXT, 
	consecutive_failures INTEGER NOT NULL, 
	in_stock BOOLEAN, 
	created_at DATETIME NOT NULL, 
	PRIMARY KEY (id), 
	UNIQUE (product_id, supermarket_id), 
	FOREIGN KEY(product_id) REFERENCES products (id) ON DELETE CASCADE, 
	FOREIGN KEY(supermarket_id) REFERENCES supermarkets (id) ON DELETE CASCADE
);

CREATE INDEX ix_product_urls_supermarket_id ON product_urls (supermarket_id);

CREATE TABLE crawl_run_items (
	id INTEGER NOT NULL, 
	run_id INTEGER NOT NULL, 
	product_url_id INTEGER, 
	url VARCHAR(1000) NOT NULL, 
	status VARCHAR(20) NOT NULL, 
	error TEXT, 
	created_at DATETIME NOT NULL, 
	PRIMARY KEY (id), 
	FOREIGN KEY(run_id) REFERENCES crawl_runs (id) ON DELETE CASCADE, 
	FOREIGN KEY(product_url_id) REFERENCES product_urls (id) ON DELETE SET NULL
);

CREATE INDEX ix_crawl_run_items_created_at ON crawl_run_items (created_at);

CREATE INDEX ix_crawl_run_items_run_id ON crawl_run_items (run_id);

CREATE TABLE crawl_tasks (
	id VARCHAR(32) NOT NULL, 
	product_url_id INTEGER NOT NULL, 
	product_id INTEGER NOT NULL, 
	supermarket_id VARCHAR(64) NOT NULL, 
	url VARCHAR(1000) NOT NULL, 
	status VARCHAR(20) NOT NULL, 
	attempts INTEGER NOT NULL, 
	created_at DATETIME NOT NULL, 
	claimed_at DATETIME, 
	lease_expires_at DATETIME, 
	completed_at DATETIME, 
	run_id INTEGER, 
	last_error TEXT, 
	PRIMARY KEY (id), 
	FOREIGN KEY(product_url_id) REFERENCES product_urls (id) ON DELETE CASCADE, 
	FOREIGN KEY(product_id) REFERENCES products (id) ON DELETE CASCADE, 
	FOREIGN KEY(supermarket_id) REFERENCES supermarkets (id) ON DELETE CASCADE, 
	FOREIGN KEY(run_id) REFERENCES crawl_runs (id) ON DELETE SET NULL
);

CREATE INDEX ix_crawl_tasks_status_created ON crawl_tasks (status, created_at);

CREATE INDEX ix_crawl_tasks_supermarket_id ON crawl_tasks (supermarket_id);

CREATE INDEX ix_crawl_tasks_product_url_id ON crawl_tasks (product_url_id);

CREATE INDEX ix_crawl_tasks_run_id ON crawl_tasks (run_id);

CREATE TABLE price_records (
	id INTEGER NOT NULL, 
	product_url_id INTEGER NOT NULL, 
	current_price INTEGER NOT NULL, 
	original_price INTEGER, 
	member_price INTEGER, 
	in_stock BOOLEAN, 
	crawled_at DATETIME NOT NULL, 
	source VARCHAR(20) NOT NULL, 
	needs_review BOOLEAN NOT NULL, 
	raw_text TEXT, 
	PRIMARY KEY (id), 
	FOREIGN KEY(product_url_id) REFERENCES product_urls (id) ON DELETE CASCADE
);

CREATE INDEX ix_price_records_url_time ON price_records (product_url_id, crawled_at);

CREATE INDEX ix_price_records_needs_review ON price_records (needs_review);

CREATE INDEX ix_price_records_crawled_at ON price_records (crawled_at);
