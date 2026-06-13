# Test Plan

This document describes the testing strategy for `bilicli`. Three test
layers exist; each corresponds to a different level of integration.

## Running tests

```bash
# All unit + integration tests (serial; storage tests share a global pool)
cargo test -- --test-threads=1

# A single module
cargo test --lib storage::cookies
cargo test --lib ipc::media
cargo test --lib backends

# With output
cargo test --lib -- --nocapture

# End-to-end tests against the real B 站 API (require login)
cargo test --lib -- --ignored
```

The `.cargo/config.toml` sets `RUST_TEST_THREADS=1` by default so
storage tests can run without cross-test pollution. Override with
`RUST_TEST_THREADS=N cargo test` if you need to run in parallel.

## Layer 1: Unit tests (40+)

Located next to the code they test (in-module `#[cfg(test)] mod tests`).

| Module | Test focus | Count |
|---|---|---|
| `error::tests` | Stable error codes, AuthError display | 2 |
| `backends::paths::tests` | `Paths::new`, env override, `dir_size` | 4 |
| `backends::http::tests` | `ProxyConfig` defaults, `build_client` no-proxy | 3 |
| `backends::sidecar::tests` | `SidecarKind` names, env-var lookup, missing-binary error | 4 |
| `ipc::shared::tests` | `Headers` default keys, `to_header_map`, `get_sec/millis`, `random_string`, `get_unique_path` | 8 |
| `ipc::storage::migrate::tests` | All tables created, idempotent | 2 |
| `ipc::storage::db::tests` | `init_creates_db_file`, idempotent | 2 |
| `ipc::storage::cookies::tests` | insert/overwrite/delete/clear/names | 5 |
| `ipc::storage::config::tests` | defaults, write/read roundtrip, dotted-path get, locale detect | 8 |
| `ipc::storage::queue::tests` | insert/load/remove/parse | 4 |
| `ipc::storage::schedulers::tests` | insert/get/remove/update | 2 |
| `ipc::storage::tasks::tests` | insert/get/update/remove, type/status roundtrip, log_event | 8 |
| `ipc::media::tests` | parse empty/bv/av/ss/ep/fid/URL variants, `FromStr`, `ResourceKind::as_str` | 22 |
| `ipc::login::tests` | `ScanLoginEvent` serialization, `stop_login` idempotent | 3 |
| `ipc::aria2c::tests` | `pick_free_port` returns valid ports, JSON deserialization | 4 |
| `ipc::ffmpeg::tests` | `MediaInfo` JSON deserialization, missing optional fields | 2 |
| `ipc::bilibili_api::tests` | `PageInfo`/`ResourceDescription` construction, `ViewApiResponse` deserialization | 3 |

## Layer 2: Integration tests (planned, in `tests/integration/`)

These run real subcommands through the binary using `assert_cmd` +
`wiremock` to mock B 站's API. They live in `tests/integration/*.rs`
and are marked `#[ignore]` until the full CLI integration is finalized.

| Test | What it does |
|---|---|
| `test_info_subcommand` | `bilicli info` exits 0 and prints version + paths |
| `test_config_show_roundtrip` | `bilicli config set` then `config get` returns the new value |
| `test_parse_url_mocked` | `wiremock` returns a fake view API response, `bilicli parse url` decodes it |
| `test_auth_status` | `bilicli auth status` returns a valid JSON object with `logged_in: false` |
| `test_db_export_import` | `bilicli db export` writes a SQLite file, `bilicli db import` re-loads it |
| `test_repl_dispatch` | Spawn the REPL on stdin, type `exit`, assert clean shutdown |

## Layer 3: End-to-end tests (planned, in `tests/e2e/`)

These exercise the real B 站 API and require a logged-in session.
Marked `#[ignore]` so they don't run on CI.

| Test | What it does |
|---|---|
| `test_login_qrcode` | `bilicli auth qrcode`, scan with a real phone, confirm `bilicli auth status` shows logged in |
| `test_parse_real_bv` | Parse a real BV id (e.g. BV1GJ411x7h7) and check the title matches the page |
| `test_parse_real_favorite` | Parse a real favorite folder |
| `test_submit_video_dry` | Submit a small (240p) video download and wait for the aria2c task to complete |
| `test_db_cross_version` | Export from CLI, restore in GUI BiliTools version, verify tasks still load |

## Mock strategy

- **No real B 站 calls** in unit or CI integration tests.
- `wiremock` is used to stand up a fake `api.bilibili.com` server for
  integration tests.
- Aria2 is tested by calling the RPC against a local `aria2c` if
  available, but those tests are gated on `which aria2c` returning
  a path; otherwise they're skipped.

## What we deliberately do not test

- The Tauri GUI (no GUI in the CLI).
- The full hand-off to `DanmakuFactory` (depends on a third-party
  binary that the user must install).
- The download progress callback (depends on a long-running
  aria2c process; integration tested in E2E only).

## Known failures / TODOs

- `tests/integration/` not yet written — slated for Phase 5.5 (next
  iteration after the 7-phase release).
- `tests/e2e/` requires a logged-in session; left as `#[ignore]`
  until a CI runner with a real phone is available.
- REPL test (`test_repl_dispatch`) is partial — only a few of the
  REPL commands are auto-dispatched today.
