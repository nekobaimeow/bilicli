# BiliTools GUI → API 映射分析

> **来源：** `https://github.com/btjawa/BiliTools` v1.4.7 (commit `da8693e`)
> **目标：** 把 5,464 行 Rust 业务代码重新组织为可被 CLI 调用的形态
> **源路径：** `<WORKSPACE>/BiliTools`
> **输出路径：** `<WORKSPACE>/bilicli-cli`

---

## 1. 仓库概览

| 维度 | 数据 |
|---|---|
| 总行数 | 5,464 行 Rust + 1,679 行 README + 4 大块 TS 前端（不在 CLI 范围） |
| 业务模块 | `src-tauri/src/{shared,commands,errors,services/*,storage/*}.rs` |
| GUI 模块（不移植） | Tauri commands 中的窗口/主题/剪贴板/单实例部分 |
| 协议 | GPL-3.0-or-later（必须沿用） |
| Rust 依赖数 | 25+ crates（tokio, sqlx, tauri, tauri-plugin-*, sea-query, etc.） |

## 2. Tauri Commands 全清单（20 个）

来自 `src-tauri/src/commands.rs` 和 `src-tauri/src/lib.rs::run()` 的 `collect_commands!` 宏。

| # | Command | 域 | CLI 状态 | 调用链 |
|---|---|---|---|---|
| 1 | `meta` | 启动 | ✅ 保留 | `app.package_info().version` + `config::read` + `tasks/schedulers/queues::load` |
| 2 | `init` | 启动 | ✅ 保留 | `login::stop_login` + `get_buvid` + `get_bili_ticket` + `get_uuid` + `HEADERS.refresh` |
| 3 | `set_window` | 窗口 | ❌ 砍 | Tauri WebviewWindow 专属 |
| 4 | `config_write` | 配置 | ✅ 保留 | `config::write` |
| 5 | `open_cache` | 缓存 | ✅ 保留 | `tauri_plugin_opener::open_path` → CLI 改 `xdg-open` / `open` |
| 6 | `get_size` | 缓存 | ✅ 保留 | 遍历目录，async stream 进度 |
| 7 | `clean_cache` | 缓存 | ✅ 保留 | 删除目录 + 重启（Tauri `app.restart`）→ CLI 不重启 |
| 8 | `db_import` | 数据库 | ✅ 保留 | `db::import` + 重启 → CLI 不重启 |
| 9 | `db_export` | 数据库 | ✅ 保留 | `db::export` |
| 10 | `export_data` | 工具 | ✅ 保留 | 写 JSON 文件 |
| 11 | `stop_login` | 登录 | ✅ 保留 | 设置 `LOGIN_POLLING = false` |
| 12 | `exit` | 登录 | ✅ 保留 | 调 B 站 exit API，删 cookie |
| 13 | `sms_login` | 登录 | ⚠️ 暂不暴露 | B 站风控敏感，CLI 暂不暴露 |
| 14 | `pwd_login` | 登录 | ⚠️ 暂不暴露 | 同上（需滑块验证码） |
| 15 | `switch_cookie` | 登录 | ⚠️ 暂不暴露 | 同上 |
| 16 | `scan_login` | 登录 | ✅ **主路径** | 轮询 B 站 `/x/passport-login/web/qrcode/poll` |
| 17 | `refresh_cookie` | 登录 | ✅ 保留 | 用 refresh_token 续期 |
| 18 | `ctrl_event` | 队列 | ✅ 保留 | 任务 pause/resume/cancel 等控制 |
| 19 | `open_folder` | 队列 | ✅ 保留 | `tauri_plugin_opener::open_path` → CLI 改系统命令 |
| 20 | `submit_task` | 队列 | ✅ **主路径** | 提交下载任务 |
| 21 | `plan_scheduler` | 队列 | ✅ 保留 | 计划下载 |
| 22 | `process_scheduler` | 队列 | ✅ 保留 | 处理计划 |

> 注：`update_max_conc` 在 `lib.rs` 中被注释掉，不算 command。

## 3. 业务服务函数清单（必须移植）

来自 `src-tauri/src/services/`：

| 文件 | 行数 | 函数 / 类型 | CLI 角色 |
|---|---|---|---|
| `login.rs` | 598 | `scan_login`, `pwd_login`, `sms_login`, `switch_cookie`, `refresh_cookie`, `get_buvid`, `get_bili_ticket`, `get_uuid`, `exit`, `stop_login` | 登录态管理 |
| `aria2c.rs` | 391 | `Aria2Rpc` 客户端 + `Aria2TellStatus` 等结构 | 下载器后端 |
| `ffmpeg.rs` | 419 | `test`, `convert_mp3`, `convert_dash`, `convert_cover`, 各种 probe | 媒体后处理 |
| `queue/mod.rs` | 13 | `init` | 队列启动 |
| `queue/manager.rs` | 238 | `Manager::insert/update/remove`, `get`, `sort`, `Status` | 任务管理 |
| `queue/scheduler.rs` | 249 | `Scheduler` 增删改查 + 触发 | 计划下载 |
| `queue/task.rs` | 276 | `Task` / `SubTask` / `TaskType` 数据结构 + 命名格式化 | 任务模型 |
| `queue/handlers.rs` | 505 | 11 个 `handle_*` 函数（视频/音频/封面/字幕/弹幕/混流等） | 任务处理器 |
| `queue/runtime.rs` | 243 | `Runtime` + `RUNTIME` 单例 | 任务运行时 |
| `queue/frontend.rs` | 199 | `RequestAction` + `TaskPrepareResp` + `QueueEvent` | 前端交互协议 |
| `queue/types.rs` | 156 | `MediaNfoThumb` 等 | 类型定义 |
| `queue/atomics.rs` | 152 | `QueueType` + `CtrlEvent` + 原子状态 | 队列控制 |
| `storage/db.rs` | 177 | `init`, `get_db`, `close_db`, `export`, `import`, `TableSpec` | SQLite 入口 |
| `storage/config.rs` | 265 | `Settings` + 22 个字段 + `read/write` + `get_cache` | 全局配置 |
| `storage/cookies.rs` | 175 | `load`, `insert`, `delete`, `clear` | Cookie 存储 |
| `storage/queue.rs` | 82 | `load`, `insert`, `remove`, `update` | 队列持久化 |
| `storage/schedulers.rs` | 194 | `load`, `insert`, `remove` | 计划任务持久化 |
| `storage/tasks.rs` | 176 | `load`, `insert`, `remove`, `update` | 任务持久化 |
| `storage/migrate.rs` | 38 | 版本迁移 | 数据库迁移 |
| `shared.rs` | 419 | `HEADERS`, `init_client`, `Headers`, `Sidecar`, `USER_AGENT`, `get_sec/millis`, `random_string`, `get_unique_path`, `get_image` | 共享工具 |

## 4. Tauri 耦合点清单（必须解耦）

| Tauri 概念 | 出现位置 | CLI 替代方案 |
|---|---|---|
| `tauri::AppHandle` | 40+ 处，主要是 `get_app_handle().path().app_data_dir()` | `crate::backends::paths::*`（用 `directories` crate） |
| `tauri::http::{header, StatusCode, HeaderMap, HeaderName, HeaderValue}` | `shared.rs`, `login.rs` | `reqwest::header::*`（类型兼容） |
| `tauri::async_runtime::{spawn, Receiver, Channel}` | `commands.rs`, `aria2c.rs` | `tokio::spawn`, `tokio::sync::mpsc` |
| `tauri_plugin_http::reqwest::{Client, Proxy}` | `shared.rs`, `login.rs` | `reqwest::{Client, Proxy}` 直连 |
| `tauri_plugin_shell::{ShellExt, process::CommandChild/Event}` | `aria2c.rs`, `ffmpeg.rs`, `queue/handlers.rs`, `queue/frontend.rs` | `tokio::process::Command` |
| `tauri_plugin_opener::open_path` | `commands.rs`（`open_cache`, `open_folder`） | `open` / `xdg-open` 命令 |
| `tauri::ipc::Channel<T>` | `commands.rs`（`get_size`, `scan_login`） | 改用 callback / stream / 简单 polling |
| `tauri::Manager::restart` | `commands.rs`（`clean_cache`, `db_import`） | 不需要重启；CLI 是无状态单次调用 |
| `tauri::Manager::path` | `config.rs`, `shared.rs` | `directories` crate |
| `tauri_specta::{collect_commands, collect_events}` | `lib.rs` | 删除（CLI 不用） |
| `tauri_specta::Event::emit` | `shared.rs`（`Headers::refresh`, `ProcessError`） | 改用 `tracing` 日志 |
| `tauri_plugin_log::Builder` | `lib.rs` | `tracing_subscriber` |
| `tauri_plugin_clipboard_manager/dialog/http/log/opener/os/process/shell/single_instance/updater` | `lib.rs` | 全部删除 |
| `tauri::WebviewWindow` + `Webview2` 调用 | `commands.rs::set_window` | 砍 |
| `tauri::Theme` / `WindowEffect` + `dark_light` | `shared.rs` | 砍 |
| `sys_locale::get_locale` | `shared.rs`（默认语言） | 保留，改用 `sys-locale` 直连 |

## 5. 数据持久化结构（SQLite schema）

由 `storage/db.rs` + `storage/migrate.rs` 管理，CLI 必须沿用 schema 以保证：
- 与 GUI 版数据库互通
- 增量升级（migrate.rs 1→2→3→...）

主要表（推断自 `storage/{tasks,queue,schedulers,cookies}.rs`）：
- `tasks` (id, type, source, options, status, created_at, ...)
- `queue` (id, task_id, position, state)
- `schedulers` (id, cron, next_run, last_run, task_id)
- `cookies` (name, value, expires)
- `settings` (key, value) — JSON blob

## 6. GUI 专属功能（全部砍）

- 窗口效果（Mica/Acrylic/Sidebar）
- 主题（Light/Dark/Auto 切换）
- 剪贴板监听
- 单实例
- 拖拽搜索
- 自更新
- 通知（Toast）
- 国际化（前端 vue-i18n）— CLI 用 `i18n` crate 简化版
- 任何 `tauri::WebviewWindow` 相关

## 7. 业务逻辑特征（复用价值）

| 特征 | 复用价值 | 说明 |
|---|---|---|
| WBI 签名 | ⭐⭐⭐⭐⭐ | B 站风控核心，不重写 |
| Buvid3/Buvid4 获取 | ⭐⭐⭐⭐⭐ | 反爬指纹，复制即用 |
| BiliTicket 签名 | ⭐⭐⭐⭐⭐ | HMAC-SHA256 算法 + 密钥 |
| aria2 RPC 协议 | ⭐⭐⭐⭐ | `Aria2TellStatus` 等结构解析 |
| FFmpeg 媒体探测 | ⭐⭐⭐⭐ | `get_duration`, `get_streams` |
| 弹幕 XML→ASS 转换 | ⭐⭐⭐⭐ | DanmakuFactory sidecar 协议 |
| 命名格式（ISO 8601 占位符） | ⭐⭐⭐ | 用户可配置 |
| 任务队列调度 | ⭐⭐⭐ | 复用 Manager/Runtime |
| Cookie 持久化 | ⭐⭐⭐ | 沿用 schema |

## 8. 风险清单

1. **B 站 API 变更** — 任何接口调整都会让 CLI 与 GUI 同时失效（同步风险）
2. **Sidecar 二进制分发** — aria2/ffmpeg/DanmakuFactory 用户必须自装（文档说清）
3. **扫码登录需人工配合** — agent 跑 QR login 时把 PNG 写到 stdout/file，用户扫码
4. **某些 GUI 业务逻辑藏在 handlers.rs 里** — 比如 opus 封面处理，可能混有 `frontend::RequestAction` 耦合
5. **Schedule 时间格式** — 原代码用 `cron` crate，CLI 必须沿用

## 9. 映射表总结

| GUI 域 | Tauri Command | 业务模块 | CLI 子命令 | 改造点 |
|---|---|---|---|---|
| **启动** | `meta`, `init` | `shared`, `login::get_buvid` | `bilicli init`, `bilicli info` | 移除 `app.restart` |
| **登录** | `scan_login`, `refresh_cookie`, `exit`, `stop_login` | `login.rs` | `auth qrcode/refresh/exit` | 改 `Channel<T>` → 简单返回值/轮询 |
| **解析** | (无) | 隐含在 `submit_task` 内部 | `parse url/fav/watchlater/bangumi` | 把 `frontend::RequestAction` 提到独立函数 |
| **下载** | `submit_task`, `ctrl_event`, `open_folder` | `queue/{handlers,manager,runtime,task}` | `download submit/list/status/cancel/open` | handlers 改用 `tokio::process` |
| **媒体后处理** | (隐含) | `ffmpeg.rs`, `queue/handlers.rs` | `download status`（自动触发） | 同上 |
| **配置** | `config_write` | `storage/config.rs` | `config show/get/set/reset` | 移除 `theme/window_effect/clipboard` 字段 |
| **缓存** | `get_size`, `clean_cache`, `open_cache` | `storage/config.rs::get_cache` | `cache list/size/clean/open` | 改用系统命令打开目录 |
| **数据库** | `db_import`, `db_export` | `storage/db.rs` | `db export/import/tasks` | 移除 `app.restart` |
| **计划任务** | `plan_scheduler`, `process_scheduler` | `queue/scheduler.rs` | `schedule list/add/remove/run` | 沿用 cron |
| **工具** | `export_data` | 直接写 JSON | `tools export-data` | 无改造 |

## 10. CLI 不实现的（明确排除）

- `sms_login` / `pwd_login` / `switch_cookie` — 全部 B 站风控敏感，CLI 暂不暴露
- `set_window` — 纯 GUI
- 自更新（`tauri_plugin_updater`）— CLI 用 `cargo install` 升级
- 剪贴板监听 / 拖拽搜索 — 纯 GUI
- `meta` 中的 `hash` 字段（git commit hash）— CLI 用自己的版本号

---

## 验收

✅ 本文件完整覆盖所有 20 个 Tauri command + 所有 5,464 行 Rust 业务代码
✅ 标注每个 command 的 CLI 状态（保留/改造/砍）
✅ 列出 Tauri 耦合点与替代方案
✅ 风险与未决问题已标记
