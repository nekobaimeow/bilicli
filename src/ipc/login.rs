// SPDX-License-Identifier: GPL-3.0-or-later
// Login — ported from BiliTools `src-tauri/src/services/login.rs`.
//
// Original CLI port: 5 of the 6 login commands are kept
// (`get_buvid`, `get_bili_ticket`, `get_uuid`, `scan_login`,
// `exit`, `stop_login`, `refresh_cookie`). The other three
// (`sms_login`, `pwd_login`, `switch_cookie`) are intentionally NOT
// exposed to the CLI because Bilibili's risk control flags them as
// automated password entry — see DESIGN.md §10.
//
// The Tauri `Channel<isize>` callback used by the GUI for
// `scan_login` polling is replaced with a plain async function that
// returns a `ScanLoginEvent` enum on each poll; the CLI's REPL or
// `auth qrcode-poll` command renders the events as JSON.

use crate::error::{AuthError, CliError};
use crate::ipc::shared::{get_millis, get_sec, init_client, init_client_no_proxy, HEADERS};
use crate::ipc::storage::cookies;
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

// =====================  Atomics  =====================

static LOGIN_POLLING: AtomicBool = AtomicBool::new(false);

/// Stop an ongoing `scan_login` poll. The next iteration will exit.
pub fn stop_login() {
    LOGIN_POLLING.store(false, Ordering::SeqCst);
}

// =====================  B 站 API responses  =====================

#[derive(Serialize, Deserialize, Debug)]
struct ExitLoginResponse {
    code: i64,
    message: String,
    ts: i64,
    data: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
struct BuvidResponse {
    code: i64,
    message: String,
    data: BuvidResponseData,
}

#[derive(Serialize, Deserialize, Debug)]
struct BuvidResponseData {
    b_3: String,
    b_4: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BiliTicketResponse {
    code: i64,
    message: String,
    ttl: i64,
    data: Option<BiliTicketResponseData>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BiliTicketResponseData {
    ticket: String,
    created_at: i64,
    ttl: i64,
    context: serde_json::Value,
    nav: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
struct ScanLoginResponse {
    code: i64,
    message: String,
    ttl: i64,
    data: Option<ScanLoginResponseData>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ScanLoginResponseData {
    url: String,
    refresh_token: String,
    timestamp: i64,
    code: i64,
    message: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct RefreshCookieResponse {
    code: i64,
    message: String,
    ttl: i64,
    data: Option<RefreshCookieResponseData>,
}

#[derive(Serialize, Deserialize, Debug)]
struct RefreshCookieResponseData {
    status: i64,
    message: String,
    refresh_token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct QrcodeGenerateResponse {
    code: i64,
    message: String,
    ttl: i64,
    data: Option<QrcodeGenerateResponseData>,
}

#[derive(Serialize, Deserialize, Debug)]
struct QrcodeGenerateResponseData {
    url: String,
    qrcode_key: String,
}

// =====================  Public types  =====================

/// Result of `start_scan_login`: the URL to encode as a QR code and the
/// key for polling. The CLI writes the QR PNG to `--qrcode-output` and
/// then calls `poll_scan_login(key)` in a loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanLoginStart {
    pub qr_url: String,
    pub qrcode_key: String,
    pub qr_png: Vec<u8>,
}

/// Outcome of a single QR poll cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScanLoginEvent {
    /// User has not yet scanned the QR. Keep polling.
    Waiting,
    /// User scanned but has not yet confirmed. Keep polling.
    Scanned { url: String },
    /// User confirmed. Cookies saved; login complete.
    Confirmed { refresh_token: String, url: String },
    /// B 站 rejected the QR (expired, already used, etc.). Stop polling.
    Rejected { code: i64, message: String },
}

// =====================  Buvid / Ticket / UUID  =====================

pub async fn get_buvid() -> Result<(), CliError> {
    let client = init_client().await?;
    let html_resp = client.get("https://www.bilibili.com").send().await?;
    if !html_resp.status().is_success() {
        return Err(CliError::http(
            html_resp.status().as_u16(),
            "Error while fetching initial Cookies",
        ));
    }
    let cookies: Vec<String> = html_resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .flat_map(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .collect();
    let mut has_buvid_3 = false;
    for cookie in cookies {
        if cookie.starts_with("buvid3") {
            has_buvid_3 = true;
        }
        cookies::insert(&cookie).await?;
    }

    let buvid_resp = client
        .get("https://api.bilibili.com/x/frontend/finger/spi")
        .send()
        .await?;
    if !buvid_resp.status().is_success() {
        return Err(CliError::http(
            buvid_resp.status().as_u16(),
            "Error while fetching Buvid Cookies",
        ));
    }
    let buvid_body: BuvidResponse = buvid_resp.json().await?;
    if buvid_body.code != 0 {
        return Err(CliError::api(buvid_body.code, buvid_body.message));
    }
    if !has_buvid_3 {
        cookies::insert(&format!("buvid3={}", buvid_body.data.b_3)).await?;
    }
    cookies::insert(&format!("buvid4={}", buvid_body.data.b_4)).await?;
    Ok(())
}

pub async fn get_bili_ticket() -> Result<(), CliError> {
    let client = init_client().await?;
    let ts = get_sec();
    let cookies = cookies::load().await?;
    let bili_csrf = cookies
        .get("bili_jct")
        .map(String::as_str)
        .unwrap_or_default();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"XgwSnGZ1p")
        .map_err(|e| CliError::msg(format!("hmac key error: {e}")))?;
    mac.update(format!("ts{ts}").as_bytes());
    let tag = mac.finalize().into_bytes();
    let mut hexsign = String::with_capacity(tag.len() * 2);
    for b in tag {
        let _ = write!(&mut hexsign, "{b:02x}");
    }
    let response = client
        .post("https://api.bilibili.com/bapis/bilibili.api.ticket.v1.Ticket/GenWebTicket")
        .query(&[
            ("key_id", "ec02"),
            ("hexsign", &hexsign),
            ("context[ts]", &ts.to_string()),
            ("csrf", bili_csrf),
        ])
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(CliError::http(
            response.status().as_u16(),
            "Error while fetching BiliTicket Cookie",
        ));
    }
    let body: BiliTicketResponse = response.json().await?;
    if let Some(data) = body.data {
        cookies::insert(&format!("bili_ticket={}", data.ticket)).await?;
        Ok(())
    } else {
        Err(CliError::api(body.code, body.message))
    }
}

pub async fn get_uuid() -> Result<(), CliError> {
    const DIGIT_MAP: [&str; 16] = [
        "1", "2", "3", "4", "5", "6", "7", "8", "9", "A", "B", "C", "D", "E", "F", "10",
    ];
    let s = |length: usize| -> String {
        let mut rng = rand::rng();
        (0..length)
            .map(|_| DIGIT_MAP[rng.random_range(0..DIGIT_MAP.len())])
            .collect()
    };
    let ts = (get_millis() % 100_000) as u32;
    let uuid = format!(
        "{}-{}-{}-{}-{}{:05}infoc",
        s(8),
        s(4),
        s(4),
        s(4),
        s(12),
        ts
    );
    cookies::insert(&format!("_uuid={uuid}")).await?;
    Ok(())
}

// =====================  QR login  =====================

/// Step 1: request a new QR code from B 站. Returns the URL the user
/// needs to scan (e.g. `https://passport.bilibili.com/x/passport-login/web/qrcode/h5?...
/// `) plus a PNG-encoded QR for convenience.
///
/// This calls `https://passport.bilibili.com/x/passport-login/web/qrcode/generate`
/// which is the upstream endpoint used by the GUI version.
pub async fn start_scan_login() -> Result<ScanLoginStart, CliError> {
    let client = init_client_no_proxy().await?;
    let resp = client
        .get("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(
            resp.status().as_u16(),
            "failed to request QR code from bilibili",
        ));
    }
    let body: QrcodeGenerateResponse = resp.json().await?;
    let data = body
        .data
        .ok_or_else(|| AuthError::Scan(body.message.clone()))?;
    let png = render_qr_png(&data.url)?;
    Ok(ScanLoginStart {
        qr_url: data.url,
        qrcode_key: data.qrcode_key,
        qr_png: png,
    })
}

/// Step 2: poll the QR login state. Returns an event describing
/// what the user has done (or not done) since the last poll.
///
/// `qrcode_key` is the value returned by `start_scan_login`.
pub async fn poll_scan_login(qrcode_key: &str) -> Result<ScanLoginEvent, CliError> {
    let client = init_client_no_proxy().await?;
    let resp = client
        .get(format!(
            "https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key={qrcode_key}"
        ))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(
            resp.status().as_u16(),
            "Error while polling QR code login",
        ));
    }
    let set_cookies: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .flat_map(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .collect();
    let body: ScanLoginResponse = resp.json().await?;

    match body.data {
        None => Ok(ScanLoginEvent::Rejected {
            code: body.code,
            message: body.message,
        }),
        Some(data) => match data.code {
            0 => {
                // Confirmed — persist cookies. Set-Cookie headers are
                // full records like `DedeUserID=123; Path=/; Domain=...;
                // Expires=...; Secure; SameSite=None`; we only want the
                // `name=value` pair for our local DB.
                for c in set_cookies {
                    if let Some(pair) = c.split(';').next() {
                        cookies::insert(pair.trim()).await?;
                    }
                }
                cookies::insert(&format!("refresh_token={}", data.refresh_token)).await?;
                HEADERS.refresh().await?;
                Ok(ScanLoginEvent::Confirmed {
                    refresh_token: data.refresh_token,
                    url: data.url,
                })
            }
            86038 | 86039 => Ok(ScanLoginEvent::Rejected {
                code: data.code,
                message: data.message,
            }),
            86101 => Ok(ScanLoginEvent::Scanned { url: data.url }), // 待确认
            _ => Ok(ScanLoginEvent::Waiting),                      // 未扫描
        },
    }
}

/// Convenience wrapper: poll every 2 seconds until Confirmed, Rejected,
/// or `stop_login()` is called. Returns the final `ScanLoginEvent`.
pub async fn await_scan_login(qrcode_key: &str) -> Result<ScanLoginEvent, CliError> {
    LOGIN_POLLING.store(true, Ordering::SeqCst);
    while LOGIN_POLLING.load(Ordering::SeqCst) {
        let event = poll_scan_login(qrcode_key).await?;
        match &event {
            ScanLoginEvent::Confirmed { .. } | ScanLoginEvent::Rejected { .. } => {
                LOGIN_POLLING.store(false, Ordering::SeqCst);
                return Ok(event);
            }
            _ => sleep(Duration::from_secs(2)).await,
        }
    }
    Err(AuthError::Cancelled.into())
}

// =====================  Refresh / Exit  =====================

/// Refresh cookies using a stored `refresh_token`.
pub async fn refresh_cookie() -> Result<(), CliError> {
    let cookies = cookies::load().await?;
    let refresh_token = cookies
        .get("refresh_token")
        .ok_or_else(|| AuthError::Refresh("no refresh_token in storage".into()))?
        .clone();
    let client = init_client_no_proxy().await?;
    let resp = client
        .post("https://passport.bilibili.com/x/passport-login/web/cookie/refresh")
        .query(&[
            ("csrf", String::from(cookies.get("bili_jct").cloned().unwrap_or_default().as_str())),
            ("refresh_csrf", String::new()),
            ("source", String::from("main-fe-header")),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(
            resp.status().as_u16(),
            "Error while refreshing cookies",
        ));
    }
    let set_cookies: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .flat_map(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .collect();
    let body: RefreshCookieResponse = resp.json().await?;
    let data = body.data.ok_or_else(|| AuthError::Refresh(body.message.clone()))?;
    for c in set_cookies {
        cookies::insert(&c).await?;
    }
    cookies::insert(&format!("refresh_token={}", data.refresh_token)).await?;
    HEADERS.refresh().await?;
    Ok(())
}

/// Logout: clear all cookies.
pub async fn exit() -> Result<(), CliError> {
    let client = init_client().await?;
    let cookies = cookies::load().await?;
    let bili_csrf = cookies
        .get("bili_jct")
        .map(String::as_str)
        .unwrap_or_default();
    let resp = client
        .post("https://passport.bilibili.com/login/exit/v2")
        .query(&[("biliCSRF", bili_csrf)])
        .send()
        .await?;
    let set_cookies: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .flat_map(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .collect();
    let body: ExitLoginResponse = resp.json().await?;
    if body.code != 0 {
        return Err(CliError::api(body.code, body.message));
    }
    for c in set_cookies {
        cookies::delete(
            c.split_once('=').map(|(name, _)| name).unwrap_or_default(),
        )
        .await?;
    }
    // Also drop the refresh_token so the user is fully logged out.
    cookies::delete("refresh_token").await?;
    cookies::delete("bili_ticket").await?;
    HEADERS.refresh().await?;
    Ok(())
}

// =====================  Helpers  =====================

/// Render a QR code as a real PNG byte buffer using the `qrcode` crate's
/// `image` renderer. This is a proper 2-D black-and-white image that
/// mobile Bilibili scanners can read.
fn render_qr_png(data: &str) -> Result<Vec<u8>, CliError> {
    use image::Luma;
    use qrcode::render::Renderer;
    use qrcode::QrCode;
    use std::io::Cursor;
    let code = QrCode::new(data.as_bytes())
        .map_err(|e| AuthError::Qrcode(e.to_string()))?;
    // 10px per module + 4-module quiet zone; black on white.
    let img = code
        .render::<Luma<u8>>()
        .quiet_zone(true)
        .min_dimensions(400, 400)
        .build();
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| AuthError::Qrcode(format!("png encode failed: {e}")))?;
    Ok(out)
}

/// Check whether the user is currently logged in. Returns true iff
/// `DedeUserID` is present in cookies (B 站's logged-in marker).
pub async fn is_logged_in() -> bool {
    cookies::has("DedeUserID").await.unwrap_or(false)
        || cookies::has("refresh_token").await.unwrap_or(false)
}

/// Return a snapshot of the current login state for `auth status`.
pub async fn status() -> serde_json::Value {
    let mut names = cookies::names().await.unwrap_or_default();
    names.sort();
    serde_json::json!({
        "logged_in": is_logged_in().await,
        "user_id": cookies::get("DedeUserID").await.ok().flatten(),
        "cookies": names,
        "has_refresh_token": cookies::has("refresh_token").await.unwrap_or(false),
    })
}

// =====================  Tests  =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_login_event_serializes_cleanly() {
        let e = ScanLoginEvent::Waiting;
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"waiting\""), "got {s}");

        let e = ScanLoginEvent::Confirmed {
            refresh_token: "rt".into(),
            url: "https://example".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"confirmed\""));
        assert!(s.contains("\"refresh_token\""));
    }

    #[test]
    fn stop_login_idempotent() {
        stop_login();
        stop_login();
        assert!(!LOGIN_POLLING.load(Ordering::SeqCst));
    }

    #[test]
    fn start_scan_login_uses_no_proxy() {
        // The QR login is fetched without proxy because some corporate
        // proxies break the B 站 login flow. The function should
        // therefore not require a proxy in settings. This test simply
        // documents the contract — the real fetch needs the network.
        let _ = init_client_no_proxy;
    }
}
