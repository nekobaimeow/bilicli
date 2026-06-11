# Review + Subtitle + Harvest 三件套 Implementation Plan

> **For Hermes:** Use the **main agent** to implement this plan task-by-task
> (3 IPC modules are tightly coupled on `danmaku::fetch_and_convert` /
> `search::search_videos`; subagents diverge on cross-file type signatures).

**Goal:** 给 `bilitools` 加 3 个能力：① 评论抓取、② 字幕抓取、③ 一条命令
"搜索关键字 → 抓前 5 个视频的弹幕 + 评论 + 字幕" 的批量入口。命令行对应
三个新子命令：`bilitools review <BV>`、`bilitools subtitle <BV>`、
`bilitools harvest <keyword> --limit 5`。

**Architecture:**
- 复用既有 `ipc::danmaku::fetch_and_convert` 和 `ipc::search::search_videos`
  作为「颗粒」，新 IPC 只实现 review / subtitle 这两块「最后一片拼图」。
- `harvest` 不调任何 HTTP — 它**只**串起 `search → danmaku → review →
  subtitle` 三个已实现的 IPC；这保证 harvest 一旦 E2E 通过，三件套都
  一定真能跑（单点 fail-fast，不会有"harvest 假绿"）。
- 评论 / 字幕 IPC 都不做 protobuf（参考 `danmaku.rs` 对 history
  弹幕的降级），少 200 行依赖和兼容性代码。

**Tech Stack:** Rust 1.81, reqwest 0.12, tokio, serde_json, serde,
  tracing, clap 4。**新增 dep：无**（全部走 std + 现有 crates）。

---

## API 探查结论（影响设计）

| API | 端点 | 匿名可拉 | 备注 |
|---|---|---|---|
| 评论 | `https://api.bilibili.com/x/v2/reply?type=1&oid={aid}&pn={pn}&ps={ps}&sort={sort}` | ✅（限 1 页 + 风控） | type=1=视频, 2=话题, 4=活动, 5=小视频, 6=直播弹幕池, 11=图片, 17=笔记 |
| 评论子评 | `https://api.bilibili.com/x/v2/reply/reply?oid={aid}&type=1&root={rpid}&pn=1&ps=20` | ✅ | 拿 sub replies |
| 字幕 | `https://api.bilibili.com/x/player/wbi/v2?bvid={bv}&cid={cid}` | ⚠️（空 list，需登录） | WBI 不强制，但 SESSDATA 必带 |
| 字幕文本 | `subtitle_url` 拿到的 JSON（.json 路径） | ✅（拿到 URL 后无门槛） | B 站是 `{body: [{from, to, content, sid}], ...}` 形态 |

**关键决定**：
- `harvest` **不**做 fan-out 并发（5 视频 × 3 串行 = 15 个请求），先串行；如
  果有性能需求再加 `tokio::spawn` 改成 fan-out（plan 不预留 YAGNI 扩展点）。
- `review` 拿不到子评时（风控/未登录）只返回 main + `degraded` 提示，不报错。
- `subtitle` 完全空 list（视频无字幕）是**正常业务结果**，不报 `degraded`。
- 字幕 URL 是 `//aisubtitle.hdslb.com/...`，HTTP 协议相对；统一补 `https:`。
- 字幕内容存 `{cid}.{lan}.json`（保留原始 B 站 JSON，不二次转 SRT，
  跟现有 `danmaku` 的"XML 直存 + 可选 ASS 转换"思路一致）。

---

## 风格规范（沿用既有 7-phase 流程）

每个 phase 结束前必须：
1. **RED**：先写 `#[test]`、跑、`cargo test --lib -p bilitools` 必看到 FAIL。
2. **GREEN**：写实现，跑到 GREEN。
3. **`cargo build` + `cargo test --lib -- --test-threads=1` 全套通过**。
4. **commit**（一个 phase = 1 commit，**不**多 commit 也**不**少 commit）。
5. **E2E 真打** B 站 API，写进 `docs/e2e-2026-06-11.md`。
6. **push** 远端。
7. 下一个 phase 才开始。

**绝对禁止**：
- 跳过 RED 直接写实现。
- "代码看起来对"就 commit — 必须有 `cargo test` 实跑输出。
- 引入新 dep（无 protobuf、无 quick-xml、无 srt 解析）。
- E2E 用 `wiremock` mock — 全部真打 B 站，mock 等价于自我欺骗。

---

## Phase 1: `ipc::review` + `cli::review`（评论）

**Files:**
- Create: `src/ipc/review.rs`
- Create: `src/cli/review.rs`
- Modify: `src/ipc/mod.rs` — 加 `pub mod review;`
- Modify: `src/cli/mod.rs` — 加 `pub mod review;`
- Modify: `src/cli/root.rs` — `Command` 加 `Review` 变体
- Modify: `src/main.rs` — `Command::Review` dispatch
- Modify: `docs/e2e-2026-06-11.md` — 加 Phase 1 验证记录

**实现要点:**
- `ReviewSource` enum: `Main{ hot | time }` 默认 `hot`；`Sub{ root_rpid }`
- `Reply { rpid, mid, uname, message, like, ctime, sub_replies, sub_replies_count, is_top }`
- `ReviewResults { bv, aid, title, total, pages, hot_count, time_count, replies, degraded }`
- `ReviewClient::fetch(bv, source, page, page_size)` 顶层 API
- **Anonymous fallback**：HEADERS 无 SESSDATA 时 → `degraded.push("匿名模式：仅可获取第 1 页热评；登录后可见全量")`
- HTML 实体解码：`&amp;` `&lt;` `&gt;` `&quot;` `&#39;` `&nbsp;`

### Task 1.1: RED — 公开类型 + HTML 解码

**Step 1**: 写 3 个测试（暂时不写实现）：
```rust
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn decode_html_amp() { assert_eq!(decode_html_entities("a&amp;b"), "a&b"); }
    #[test] fn decode_html_quote() { assert_eq!(decode_html_entities("&quot;x&quot;"), "\"x\""); }
    #[test] fn decode_html_nbsp() { assert_eq!(decode_html_entities("a&nbsp;b"), "a\u{00a0}b"); }
}
```
跑 `cargo test --lib -p bilitools ipc::review::tests::decode_html` → 期望 FAIL（类型未定义）。

**Step 2**: 写最小 `Reply`、`ReviewSource`、`ReviewResults` 类型定义 + `decode_html_entities` 函数（用一行 `replace` 链）。跑测试 → 期望 GREEN。

**Step 3**: commit `feat(review): add ipc module skeleton + html decode`

### Task 1.2: RED — `Reply` 解析

**Step 1**: 写 3 个测试用真实 API 响应的 fixture JSON：
```rust
const FIXTURE: &str = r#"{"code":0,"message":"0","ttl":1,"data":{"page":{"count":396,"size":20,"num":1,"acount":396},"replies":[{"rpid":277505236320,"oid":115071804051527,"type":1,"mid":222222,"root":0,"parent":0,"dialog":0,"count":3,"rcount":3,"state":0,"fansgrade":1,"attr":0,"ctime":1715000000,"rpid_str":"277505236320","like":247,"action":0,"content":{"message":"<em>foo</em>","plat":1,"device":"","members":[]},"replies":null,"assist":0,"folder":{"has_folded":false,"is_folded":false,"rule":"https://www.bilibili.com/blackboard/topic_list.html"},"up_action":{"like":false,"reply":false},"stats":{},"member":{"mid":"222222","uname":"墨菲特","avatar":"https://...","level_info":{"level":6}}}]}}"#;
#[test] fn parse_single_reply_extracts_uname_like_message() { ... }
#[test] fn parse_replies_with_sub_replies_recurses() { ... }
#[test] fn parse_empty_data_returns_empty_vec() { ... }
```
跑 → FAIL（函数未实现）。

**Step 2**: 写 `parse_replies_value(&serde_json::Value) -> Vec<Reply>`（递归处理 `replies[].replies[]` 字段）。

**Step 3**: 跑 → GREEN。

**Step 4**: commit `feat(review): parse reply payload + nested sub replies`

### Task 1.3: RED — `fetch_main`

**Step 1**: 写 1 个集成测试标 `#[ignore]`（不跑 — 没有 SESSDATA）：
```rust
#[tokio::test] #[ignore] async fn fetch_main_against_real_bilibili() {
    let r = ReviewClient::fetch_main("BV1CZEY67E8o", "hot", 1, 5).await.unwrap();
    assert!(r.total > 0);
    assert!(!r.replies.is_empty());
}
```
跑（带 `--ignored`）→ 验它真能跑通。

**Step 2**: 写 `fetch_main(bv: &str, sort: &str, pn: u32, ps: u32) -> Result<ReviewResults>`：
- 先 `ipc::danmaku::fetch_view(bv)` 拿 aid + title + cid
- 再 `GET /x/v2/reply?type=1&oid={aid}&pn={pn}&ps={ps}&sort={2 if hot else 0}`
- `init_client()` 自动装 cookie
- 验 `code == 0`，parse `data.replies`

**Step 3**: 标 `#[ignore]` 跑（必须成功，否则不算 GREEN）：
```bash
cargo test --lib -p bilitools ipc::review::tests::fetch_main_against_real_bilibili -- --ignored --nocapture
```

**Step 4**: commit `feat(review): fetch hot/time main replies from bilibili`

### Task 1.4: RED — `fetch_sub`

**Step 1**: 写 1 个 `#[ignore]` 测试 `fetch_sub_against_real_bilibili` → FAIL。

**Step 2**: 写 `fetch_sub(bv, root_rpid, pn, ps)` 走 `/x/v2/reply/reply`。

**Step 3**: 跑 `--ignored` → GREEN。

**Step 4**: commit `feat(review): fetch sub-replies for a given rpid`

### Task 1.5: CLI 子命令

**Step 1**: 写 1 个测试 `render_reply_table_basic` 测表格渲染（无网络，纯函数）。

**Step 2**: 写 `cli/review.rs::run()`：
- clap 参数：`review <input> [--sort hot|time] [--page 1] [--ps 20] [--sub RPID]`
- human 模式：`rpid  uname                likes  ctime         message`
- json 模式：完整 `ReviewResults` 对象
- anonymous fallback：未登录时打印 warn

**Step 3**: 修改 `cli/root.rs` 加 `Command::Review { input, sort, page, ps, sub, no_login_warn }`。

**Step 4**: 修改 `main.rs` dispatch。

**Step 5**: 跑 `cargo build` + `cargo test --lib`（**全套**，确保没破其他测试）。

**Step 6**: E2E `bilitools review BV1CZEY67E8o --sort hot --ps 3` → 期待看到 3 行评论（匿名能拿）。

**Step 7**: commit `feat(cli): add review subcommand`.

### Task 1.6: 推远端 + 文档

- 推 `git push origin master`
- `docs/e2e-2026-06-11.md` 加 Phase 1 段落：截 5 行输出 + 评论数验证。

---

## Phase 2: `ipc::subtitle` + `cli::subtitle`（字幕）

**Files:**
- Create: `src/ipc/subtitle.rs`
- Create: `src/cli/subtitle.rs`
- Modify: `src/ipc/mod.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/cli/root.rs`
- Modify: `src/main.rs`
- Modify: `docs/e2e-2026-06-11.md`

**实现要点:**
- `SubtitleEntry { id: i64, lan: String, lan_doc: String, is_lock: bool, subtitle_url: String, type_code: i32, ai_status: i32 }`
- `SubtitleList { bv, cid, title, entries, fetched: Vec<FetchedSubtitle> }`
- `FetchedSubtitle { lan, lan_doc, path: PathBuf, body_len: usize }`
- `SubtitleClient::list(bv) -> SubtitleList`：从 `player/wbi/v2` 拿字幕元数据
- `SubtitleClient::download(entry, output_dir) -> FetchedSubtitle`：拉 `subtitle_url` JSON 直存
- `SubtitleClient::fetch_all(bv, output_dir) -> SubtitleList`：list + 批量 download

**降级策略:**
- 字幕对匿名几乎都是空 list → 不报 error，正常返回 0 entries。
- `subtitle_url` 拉失败（403/404/网络）→ 跳过那条 + `degraded`。

### Task 2.1: RED — 类型 + URL 拼接

**Step 1**: 测试 `normalize_subtitle_url`：
```rust
#[test] fn normalizes_protocol_relative() {
    assert_eq!(normalize_subtitle_url("//aisubtitle.hdslb.com/123.json"), "https://aisubtitle.hdslb.com/123.json");
}
#[test] fn keeps_https_unchanged() {
    assert_eq!(normalize_subtitle_url("https://x"), "https://x");
}
#[test] fn keeps_http_unchanged() {
    assert_eq!(normalize_subtitle_url("http://x"), "http://x");
}
```
跑 → FAIL（fn 未定义）。

**Step 2**: 写 `normalize_subtitle_url` + 公开类型。

**Step 3**: 跑 → GREEN。

**Step 4**: commit `feat(subtitle): add ipc module skeleton + url normalize`.

### Task 2.2: RED — `list`

**Step 1**: `#[ignore]` 测试 `list_against_real_bilibili` 跑 `BV1CZEY67E8o`（无字幕场景，期待 0 entries 不报错）。**同时**也跑一个已知有字幕的视频（待 E2E 时现找）。

**Step 2**: 写 `list(bv) -> SubtitleList`：调 `/x/player/wbi/v2?bvid={bv}&cid={cid}`，从 `data.subtitle.list` 读 entries（如果 `list` 是空 list，就返回空 entries，**不**是错误）。

**Step 3**: 跑 `--ignored` → GREEN（至少 empty case 跑通）。

**Step 4**: commit `feat(subtitle): list subtitle metadata from player/wbi/v2`.

### Task 2.3: RED — `download` + `fetch_all`

**Step 1**: 测试 `download_writes_file_to_output_dir` 用 wiremock（这次**允许**wiremock — 字幕文件本体是 B 站自建 CDN，不适合每跑都真打，且无登录）。

加 dev-dep 已经在既有 `Cargo.toml` 了？看（**plan 执行时查**）。

如果不许 wiremock：用 tempdir + httpbin 假 URL 风格，或者直接用 `https://httpbin.org/json` 走通（公网稳态）。**建议**：用一个 `download_to_path` 纯函数 + URL 做参数化测试；E2E 真打 B 站字幕（待 Phase 2.3 step 4 实跑验）。

**Step 2**: 写 `download(entry, output_dir) -> FetchedSubtitle`：补 `https:` → GET → 写 `{cid}.{lan}.json`。

**Step 3**: `fetch_all(bv, output_dir)` 串起 list + 每个 entry 都 download。

**Step 4**: E2E 真找一有字幕的视频跑（用户登录后再做 E2E 验）。

**Step 5**: commit `feat(subtitle): download and fetch_all`.

### Task 2.4: CLI

`bilitools subtitle <input> [--output-dir DIR] [--lan zh-Hans|...]`：
- 没字幕 → `[info] no subtitles available for this video`（不报错，exit 0）
- 有字幕 → 列出每个 entry + 文件路径

### Task 2.5: 推远端 + 文档

---

## Phase 3: `cli::harvest`（批量）

**Files:**
- Create: `src/cli/harvest.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/cli/root.rs` — `Command::Harvest { keyword, limit, output_dir, with_danmaku, with_review, with_subtitle }`
- Modify: `src/main.rs`
- Modify: `docs/e2e-06-11.md`

**实现要点:**
- 串行（先简单）— `--concurrency` 留接口但不实现（YAGNI）
- 默认开 `with_danmaku=true, with_review=true, with_subtitle=true`
- 输出策略：每个视频一个子目录 `{output_dir}/{slug-title}/`，里面放：
  - `{cid}.xml`（弹幕）
  - `{cid}.zh-Hans.json`（字幕，如有）
  - `review.json`（评论 summary + 前 5 条）
  - `meta.json`（视频元信息）
- 进度：human 模式逐步打 `[1/5] BV1xxx ... ok 312 danmaku, 3 reviews, 1 subtitle`
- 失败软处理：单个视频失败 → `degraded` 列表，下一个继续，exit 0
- 整体进度：`degraded` 空 → exit 0，否则 exit 0 + 警告

### Task 3.1: RED — pure helpers

**Step 1**: 测试 `slugify_title`：
```rust
#[test] fn slugify_chinese() { assert_eq!(slugify_title("【4KHDR】《最佳》— ULTRA 60帧"), "4khdr_ulta_60"); }
#[test] fn slugify_short() { assert_eq!(slugify_title("hi"), "hi"); }
#[test] fn slugify_truncates() { let s = "a".repeat(200); assert!(slugify_title(&s).len() <= 80); }
```

**Step 2**: 写 helper。

**Step 3**: 跑 → GREEN。

**Step 4**: commit `feat(harvest): slugify helper`.

### Task 3.2: RED — 单视频 harvest 流程

**Step 1**: `#[ignore]` 集成测试 `harvest_one_video_danmaku_and_review_against_real` → FAIL。

**Step 2**: 写 `harvest_one_video(bv, opts, ctx) -> HarvestEntry`。

**Step 3**: 跑 `--ignored` → GREEN。

**Step 4**: commit `feat(harvest): single-video harvest pipeline`.

### Task 3.3: CLI

`bilitools harvest <keyword> [--limit 5] [--output-dir ./harvest] [--no-danmaku] [--no-review] [--no-subtitle]`。

### Task 3.4: E2E + 推

- E2E：`bilitools harvest "黄金" --limit 5 --output-dir /tmp/harvest-test` → 期待 5 个子目录
- 删 `/tmp/harvest-test`
- 推 + 文档。

---

## Phase 4: 收尾

- [ ] `cargo test --lib -- --test-threads=1` 整套跑（目标 156+ 测试，Phase 1 估 +8，Phase 2 估 +6，Phase 3 估 +4）
- [ ] `cargo build --release` 出新 binary
- [ ] `git log --oneline | head -20` 自查提交链（应该 +3 commits）
- [ ] `git push origin master`
- [ ] `docs/e2e-2026-06-11.md` 顶部加 summary table
- [ ] 更新 `README.md`（如果存在）加 3 个新命令 example
- [ ] 提给用户 E2E 完整输出 + 新 binary 路径

---

## 测试统计目标

| 阶段 | 当前 | 估增 | 累计 |
|---|---|---|---|
| baseline | 142 | 0 | 142 |
| Phase 1 | | +8 | 150 |
| Phase 2 | | +6 | 156 |
| Phase 3 | | +4 | 160 |

**所有数字是预估；实际以跑通为准 — 不允许凑数。**

---

## 风险 / 已记录

1. **未登录拿不到字幕**（已验）— harvest 字幕部分对未登录用户永远空。
2. **评论风控**（已验匿名可拿但限 1 页 3-5 条）— harvest 默认 `ps=20`，未登录降级到 `ps=3`。
3. **`/x/player/wbi/v2` 是新接口** — 旧 `subtitle` 端点已死。新接口对 wbi 不强制签名但 SESSDATA 必带。
4. **不允许 subagent 介入** — 3 个 IPC 共享很多类型假设，main agent 串行做。
5. **harvest 串行慢** — 5 视频 × (弹幕 1 请求 + 评论 1 请求 + 字幕 1 请求) ≈ 15 请求；可接受（< 10s）。

---

## 不做（YAGNI）

- ❌ 字幕 → SRT/ASS 转换（B 站 JSON 已经很好了，重复劳动）
- ❌ 评论递归 sub-sub（3 层就够）
- ❌ 并发 fan-out（性能可接受）
- ❌ 评论情感分析、关键词提取
- ❌ 字幕搜索（用户没要）
- ❌ 写 cookie/db 缓存抓到的内容
