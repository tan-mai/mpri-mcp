<!-- TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host -->
# mprimcp (crate)

The Rust source of MPriMCP. User documentation: [../README.md](../README.md) ·
[../README.vi.md](../README.vi.md) · agents: [../AGENTS.md](../AGENTS.md) ·
tool parity with the Python release: [docs/parity.md](docs/parity.md) ·
deployment: [deploy/VPS_SETUP.md](deploy/VPS_SETUP.md).

```bash
cargo test                                  # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo run -- stdio                          # or serve / dashboard
```

Layout: `src/services` (business logic shared by MCP tools and the dashboard), `src/mcp` (rmcp server,
tools, `init` prompt), `src/auth` (accounts, OAuth 2.1), `src/web` (axum dashboard, maud templates),
`migrations` (SQLite schema), `assets` (server instructions, prompt, static files).
`assets/instructions.md` is the single source of the server instructions; `../AGENTS.md` must contain
it verbatim (checked by `tests/system.rs`).
