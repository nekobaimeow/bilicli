---
name: bilitools
description: Use when the user wants to download, parse, or manage B 站 (Bilibili) resources from the command line. Triggers on "下载 B 站视频", "bilibili download", "B 站工具", "bilitools", "B 站 cli", "扫二维码登录 bilibili", "解析 BV 号".
---

# bilitools — A Bilibili CLI

`bilitools` is the command-line port of [BiliTools](https://github.com/btjawa/BiliTools).
It exposes the same 5,464-line Rust backend (WBI signing, Buvid fingerprint, aria2 RPC,
FFmpeg post-processing, queue scheduling, SQLite persistence) without the Tauri GUI.

## When to use this skill

- The user wants to download a Bilibili video, audio, danmaku, or subtitle.
- The user wants to parse a B 站 URL into a structured resource descriptor.
- The user wants to manage a B 站 download queue (list / cancel / pause / retry).
- The user wants a QR-code login to bilibili.com without opening a browser.
- The user wants a one-line B 站 resource lookup in JSON.

## When NOT to use this skill

- The user wants to **upload** to B 站 (this tool is download-only).
- The user wants a **GUI** — use the original BiliTools instead.
- The user is operating on a website other than bilibili.com.

## Inputs

A `bilitools` invocation looks like:

```bash
bilitools [global-flags] <subcommand> [args]
```

Global flags (always available):
- `-j, --json` — emit JSON instead of human-readable text
- `--data-dir <DIR>` — override `BILITOOLS_DATA_DIR`
- `--log-level <LEVEL>` — `trace|debug|info|warn|error`
- `--no-color`
- `--doctor` — run health check before executing

## Outputs

**Success (default human mode):**
```
bilitools v1.4.7-cli.1
type 'help' for commands. Ctrl-D or 'exit' to quit.
```

**Success (JSON mode):**
```json
{
  "ok": true,
  "data": { ... command-specific payload ... }
}
```

**Error (any mode):**
```json
{
  "ok": false,
  "error": {
    "code": "NOT_LOGGED_IN",
    "message": "请先运行 `bilitools auth qrcode`"
  }
}
```

## Available subcommands

| Subcommand | Purpose |
|---|---|
| `info` | Show version, paths, build info |
| `init` | Initialize buvid3/buvid4/ticket/uuid (run once after install) |
| `auth qrcode` | Generate a QR PNG, output the URL + key |
| `auth qrcode-poll <key>` | Poll QR login state once (Waiting/Scanned/Confirmed/Rejected) |
| `auth status` | Print current login state |
| `auth refresh` | Manually refresh cookies |
| `auth exit` | Logout (clear cookies) |
| `parse url <URL>` | Classify any B 站 URL |
| `parse bv <BV>` / `parse av <AV>` | Parse BV/av id |
| `parse bangumi <SS>` / `parse episode <EP>` | Parse season/episode |
| `parse fav <FID>` / `parse watchlater` / `parse user <MID>` | Other resources |
| `download submit <URL>` | Submit a download task |
| `download batch <FILE>` | Submit a batch (one URL per line) |
| `download list` | List all tasks |
| `download show <ID>` | Show task details |
| `download cancel / pause / resume / retry <ID>` | Control a task |
| `schedule list / add / remove / run` | Cron-scheduled downloads |
| `config show / get / set / reset / path` | Config management |
| `cache list / size / clean / open` | Cache management |
| `db export / import / tasks` | Database management |
| `doctor` | Health check |
| `repl` | Interactive shell (default if no subcommand) |

## Workflow example

```bash
# 1. First-time setup
bilitools init

# 2. Login via QR
bilitools --json auth qrcode --output /tmp/qr.png | tee /tmp/qr.json
#    — read qrcode_key from the output JSON, then:
bilitools auth qrcode-poll $(jq -r .data.qrcode_key /tmp/qr.json)
#    — repeat until status: Confirmed
#    — user scans with B 站 phone app

# 3. Verify
bilitools --json auth status | jq

# 4. Parse a resource
bilitools --json parse bv BV1xx411c7mD

# 5. Submit download
bilitools --json download submit BV1xx411c7mD

# 6. Watch progress
bilitools download list
```

## Common errors

| Code | Meaning | Recovery |
|---|---|---|
| `MISSING_DEPENDENCY` | aria2c/ffmpeg/DanmakuFactory not found | `apt install aria2 ffmpeg` or set `sidecar.<name>` in config |
| `NOT_LOGGED_IN` | No DedeUserID cookie | Run `bilitools auth qrcode` first |
| `INVALID_URL` | Not a recognizable B 站 URL | Use `BV1xx...` / `av170001` / `ss12345` / `ep12345` form |
| `TASK_NOT_FOUND` | Bad task id | Run `bilitools download list` to see valid ids |
| `NETWORK` | Network unreachable | Check connection or proxy settings |

## Interop with the GUI

`bilitools` reads and writes the same SQLite database as the GUI BiliTools
version. The path is identical across both tools:

- Linux:   `$XDG_DATA_HOME/com.btjawa.bilitools/Storage/storage.db`
- macOS:   `~/Library/Application Support/com.btjawa.bilitools/Storage/storage.db`
- Windows: `%AppData%\com.btjawa.bilitools\Storage\storage.db`

You can run `bilitools db export` from one and `bilitools db import` on the
other to migrate state. Cookies, tasks, schedulers, and settings all
transfer.

## Hard rules

- **Never** re-implement B 站 WBI signing or Buvid fingerprint generation
  from scratch — call the existing helpers in `ipc::login`.
- **Never** hard-code a B 站 cookie or login URL — cookies are user-specific
  and must come from the user's QR login.
- **Never** call `bilitools download submit` on a `ss`/`ep` ID without
  running `bilitools parse` first to confirm the resource is reachable.
- **Never** assume `~/.local/share/com.btjawa.bilitools/Storage/storage.db`
  is the only database — always use `db::db_path()` or the `--data-dir` flag.

## Related tools

- `bilitools-cli` (this project) — command-line interface
- [BiliTools GUI](https://github.com/btjawa/BiliTools) — desktop GUI
- [CLI-Hub](https://hkuds.github.io/CLI-Anything/) — registry of CLI-Anything harnesses
