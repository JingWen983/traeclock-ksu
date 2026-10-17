//! Qoder check-in + device-flow OAuth — port of `tasks/qoder_common.rs`
//! (headers / creds / refresh), `tasks/qoder_checkin.rs` (daily dual
//! activity), `tasks/qoder_device.rs` (device profile pool) and
//! `commands/qoder/oauth.rs` (device flow).
//!
//! Known upstream constraint preserved: the server buckets accounts by
//! device-identity truth values. Derived values land in an isolated bucket
//! where `campaigns` shows no CLAIMABLE items and claim 503s — so real values
//! win over derived ones whenever present, and each account keeps ONE stable
//! device profile forever (never regenerate on failure).

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest as _;

use crate::config::AppConfig;
use crate::envelope::dig;
use crate::events::{Event, Sink};
use crate::http;
use crate::store::{Account, Store, APP_QODER};
use crate::util::{random_hex, sha256_hex};

pub const OPEN_API_BASE: &str = "https://openapi.qoder.com.cn";
pub const WEB_BASE: &str = "https://qoder.cn";
pub const COSY_CLIENT_TYPE: &str = "10";
pub const CLIENT_USER_AGENT: &str = "Qoder";
pub const CLIENT_COSY_VERSION: &str = "0.4.3";
/// Desktop value was "x86_64_win32"; android flavor default (overridable).
pub const CLIENT_MACHINE_OS_ANDROID: &str = "aarch64_android";
pub const CLIENT_MACHINE_OS_DESKTOP: &str = "x86_64_win32";

/// Poll tuning (device-flow OAuth; 1s interval per the R-10 capture).
pub const POLL_INTERVAL_MS: u64 = 1_000;
pub const POLL_TIMEOUT_MS: u64 = 180_000;

// ── creds & device profile ──────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QoderCreds {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub refresh_expires_at_ms: Option<i64>,
    /// 凭证种类：client（设备流抓包同款）
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub machine_token: String,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub nickname: String,
}

impl QoderCreds {
    pub fn from_value(v: Option<&Value>) -> QoderCreds {
        let v = v.cloned().unwrap_or(Value::Null);
        QoderCreds {
            access_token: dig(&v, &["access_token", "accessToken"]).and_then(|x| x.as_str()).unwrap_or("").into(),
            refresh_token: dig(&v, &["refresh_token", "refreshToken"]).and_then(|x| x.as_str()).unwrap_or("").into(),
            expires_at_ms: dig(&v, &["expires_at_ms", "expiresAtMs"]).and_then(|x| x.as_i64()),
            refresh_expires_at_ms: dig(&v, &["refresh_expires_at_ms", "refreshExpiresAtMs"]).and_then(|x| x.as_i64()),
            kind: dig(&v, &["kind"]).and_then(|x| x.as_str()).unwrap_or("").into(),
            machine_id: dig(&v, &["machine_id", "machineId"]).and_then(|x| x.as_str()).unwrap_or("").into(),
            machine_token: dig(&v, &["machine_token", "machineToken"]).and_then(|x| x.as_str()).unwrap_or("").into(),
            uid: dig(&v, &["uid", "userId"]).and_then(|x| x.as_str()).unwrap_or("").into(),
            nickname: dig(&v, &["nickname"]).and_then(|x| x.as_str()).unwrap_or("").into(),
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Per-account stable device profile (port of `QoderDeviceProfile`):
/// generated once per account, persisted forever, never regenerated.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceProfile {
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub machine_code: String,
    #[serde(default)]
    pub machine_type: String,
}

impl DeviceProfile {
    /// Deterministic per-uid generation: stable across reinstalls by design
    /// (uid-derived, not random) — the account keeps one identity forever.
    pub fn generate(uid: &str) -> DeviceProfile {
        let h = sha256_hex(&format!("qoder-device:{uid}"));
        DeviceProfile {
            hostname: format!("PC-{}", &h[..8].to_ascii_uppercase()),
            machine_code: h[8..26].to_string(),  // 18 hex chars
            machine_type: h[26..43].to_string(), // 17 hex chars
        }
    }
}

/// Headers (port of `build_auth_headers`): Bearer + Cosy-ClientType + UA +
/// machine id/token passthrough + device description trio.
pub fn build_auth_headers(cfg: &AppConfig, creds: &QoderCreds) -> Vec<(String, String)> {
    let mut h = vec![
        ("Authorization".to_string(), format!("Bearer {}", creds.access_token)),
        ("Cosy-ClientType".to_string(), COSY_CLIENT_TYPE.to_string()),
        ("User-Agent".to_string(), CLIENT_USER_AGENT.to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    if !creds.machine_id.is_empty() {
        h.push(("Cosy-MachineId".to_string(), creds.machine_id.clone()));
    }
    if !creds.machine_token.is_empty() {
        h.push(("Cosy-MachineToken".to_string(), creds.machine_token.clone()));
    }
    let machine_os = if cfg.platform_flavor == "windows" {
        CLIENT_MACHINE_OS_DESKTOP
    } else {
        CLIENT_MACHINE_OS_ANDROID
    };
    for (k, v) in [
        ("Cosy-Version", CLIENT_COSY_VERSION.to_string()),
        ("Cosy-MachineOS", machine_os.to_string()),
        ("Cosy-MachineHostname", hostname_of(creds)),
    ] {
        if !h.iter().any(|(hk, _)| hk.eq_ignore_ascii_case(k)) {
            h.push((k.to_string(), v));
        }
    }
    if !creds.machine_id.is_empty() {
        if !h.iter().any(|(hk, _)| hk == "Cosy-MachineCode") {
            h.push(("Cosy-MachineCode".to_string(), creds.machine_id.chars().take(18).collect()));
        }
        if !h.iter().any(|(hk, _)| hk == "Cosy-MachineType") {
            h.push(("Cosy-MachineType".to_string(), creds.machine_id.chars().take(17).collect()));
        }
    }
    cfg.apply_overrides("qoder", &mut h);
    h
}

/// Hostname: profile value (stable) wins over the ambient host name
/// (the desktop used COMPUTERNAME / "PC" fallback).
fn hostname_of(creds: &QoderCreds) -> String {
    if !creds.nickname.is_empty() && creds.nickname.starts_with("PC-") {
        return creds.nickname.clone();
    }
    "PC".to_string()
}

// ── refresh ──────────────────────────────────────────────────────────────────

/// One refresh attempt → (new_creds, rejected_by_server).
/// Only a server rejection (non-zero code / auth 4xx) marks the token dead;
/// network failures merely count (matches the source's failure taxonomy).
pub fn refresh_token_once_ex(
    agent: &ureq::Agent,
    cfg: &AppConfig,
    creds: &QoderCreds,
) -> (Option<QoderCreds>, bool) {
    if creds.refresh_token.is_empty() {
        return (None, false);
    }
    let mut h = build_auth_headers(cfg, creds);
    h.push(("Authorization".to_string(), "Bearer".to_string())); // refreshed pre-auth endpoint drops the stale bearer
    h.retain(|(k, _)| k != "Authorization");
    let (status, body, _raw) = http::post_json(
        agent,
        &format!("{OPEN_API_BASE}/device/token/refresh"),
        &h,
        &json!({ "refreshToken": creds.refresh_token }),
    );
    if status == 0 {
        return (None, false);
    }
    if (400..500).contains(&status) {
        return (None, true);
    }
    if status != 200 {
        return (None, false);
    }
    let Some(b) = body else { return (None, false) };
    let acc = dig(&b, &["accessToken", "access_token"]).and_then(|v| v.as_str()).unwrap_or("");
    if acc.is_empty() {
        return (None, false);
    }
    let mut out = creds.clone();
    out.access_token = acc.to_string();
    let rt = dig(&b, &["refreshToken", "refresh_token"]).and_then(|v| v.as_str()).unwrap_or("");
    if !rt.is_empty() {
        out.refresh_token = rt.to_string();
    }
    if let Some(ms) = dig(&b, &["expiresIn", "expires_in"]).and_then(|v| v.as_i64()) {
        out.expires_at_ms = Some(chrono::Utc::now().timestamp_millis() + ms * 1000);
    }
    (Some(out), false)
}

/// Lazy freshness gate: access_token remaining > 24h → skip.
pub fn need_refresh(creds: &QoderCreds, now_ms: i64) -> bool {
    match creds.expires_at_ms {
        Some(exp) => (exp - now_ms) as f64 / 3_600_000.0 <= 24.0,
        None => true,
    }
}

// ── device-flow OAuth (byte-faithful port of tasks/qoder_oauth.rs,
//    R-10 packet capture 2026-09-27) ─────────────────────────────────────────

/// 授权页基址
pub const DEVICE_AUTH_BASE: &str = "https://qoder.cn/device/selectAccounts";
/// 授权页 client_id（官方注册常量，新版 Qoder CN 客户端 authClientIds.prod）。
/// 随机 client_id 会被授权页判「参数无效」；compat 模式省略该参数（社区实现同款）。
pub const DEVICE_AUTH_CLIENT_ID: &str = "732aef47-9cf2-46a2-95fe-4cebb5d0d1fa";

/// expires_in 归一：>2_592_000（30 天秒数）视为毫秒原样，否则视为秒 ×1000
pub fn normalize_expires_in(e: i64) -> i64 {
    if e > 2_592_000 {
        e
    } else {
        e * 1000
    }
}

/// 一次设备流会话（nonce/verifier/machine_id 全程自持，与源码同构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFlow {
    pub nonce: String,
    pub verifier: String,
    pub machine_id: String,
    /// compat 模式（None）时不携带 client_id
    pub client_id: Option<String>,
    /// 授权页完整 URL（challenge = BASE64URL_NO_PAD(SHA256(verifier))）
    pub auth_url: String,
}

fn new_uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn uuid_v4_simple() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl DeviceFlow {
    /// 常规模式：携带官方注册固定 client_id（与真实客户端跳转逐字一致）
    pub fn new() -> Self {
        Self::build(true)
    }

    /// 兼容模式：不带 client_id（官方常量被轮换导致「参数无效」时的恢复路径）
    pub fn new_compat() -> Self {
        Self::build(false)
    }

    fn build(with_client_id: bool) -> Self {
        // verifier：双 uuid v4 simple 拼接 = 64 hex 字符（源码同款）
        let verifier = format!("{}{}", uuid_v4_simple(), uuid_v4_simple());
        let nonce = new_uuid_v4();
        let machine_id = new_uuid_v4();
        let client_id = with_client_id.then(|| DEVICE_AUTH_CLIENT_ID.to_string());
        // challenge：BASE64URL_NO_PAD(SHA256(verifier))（PKCE S256）
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        let mut url = format!(
            "{DEVICE_AUTH_BASE}?challenge={challenge}&challenge_method=S256&nonce={nonce}&machine_id={machine_id}"
        );
        if let Some(cid) = &client_id {
            url.push_str(&format!("&client_id={cid}"));
        }
        Self { nonce, verifier, machine_id, client_id, auth_url: url }
    }
}

impl Default for DeviceFlow {
    fn default() -> Self {
        Self::new()
    }
}

/// verifier 进 query 前的最小转义（正常为纯 hex，仅防御性兜底，源码同款）
fn urlencode(s: &str) -> String {
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('~', "%7E")
}

/// 单次轮询：GET /api/v1/deviceToken/poll（无 Authorization 头，与客户端一致）。
/// pending = HTTP 404 {"errorCode":"NotFound"}；成功 = HTTP 200。
pub fn device_flow_poll_once(agent: &ureq::Agent, flow: &DeviceFlow) -> (i32, Option<Value>) {
    let url = format!(
        "{}/api/v1/deviceToken/poll?nonce={}&verifier={}&challenge_method=S256",
        OPEN_API_BASE,
        flow.nonce,
        urlencode(&flow.verifier)
    );
    let headers = vec![
        ("User-Agent".to_string(), CLIENT_USER_AGENT.to_string()),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    let (status, parsed, _raw) = http::get_json(agent, &url, &headers);
    (status, parsed)
}

/// 轮询成功响应 → (凭证, uid)。
/// nonce 回验：响应 nonce 与本会话 flow.nonce 必须一致（防会话混淆）；
/// token 候选 token/accessToken/access_token（dt- 前缀）；
/// expires_in 毫秒级归一（2591999999 ≈ 30d），refresh_token_expires_in ≈360d。
pub fn parse_poll_success(body: &Value, expected_nonce: &str) -> Option<(QoderCreds, String)> {
    let resp_nonce = dig(body, &["nonce"]).and_then(|v| v.as_str()).unwrap_or("");
    if resp_nonce != expected_nonce {
        return None;
    }
    let token = dig(body, &["token", "accessToken", "access_token"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return None;
    }
    let uid = dig(body, &["user_id", "userId", "uid"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let expires_at_ms = dig(body, &["expires_in", "expiresIn"])
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse::<i64>().ok()))
        .map(|e| now_ms + normalize_expires_in(e));
    let refresh_expires_at_ms = dig(body, &["refresh_token_expires_in", "refreshTokenExpiresIn"])
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse::<i64>().ok()))
        .map(|e| now_ms + normalize_expires_in(e));
    let refresh_token =
        dig(body, &["refresh_token", "refreshToken"]).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some((
        QoderCreds {
            access_token: token,
            refresh_token,
            expires_at_ms,
            refresh_expires_at_ms,
            kind: "client".into(),
            uid: uid.clone(),
            ..Default::default()
        },
        uid,
    ))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ── check-in round ───────────────────────────────────────────────────────────

/// Campaign list → (campaign_id, kind) pairs claimable by this account.
fn fetch_campaigns(agent: &ureq::Agent, cfg: &AppConfig, creds: &QoderCreds) -> Vec<(String, String)> {
    let h = build_auth_headers(cfg, creds);
    let (status, body, _raw) = http::post_json(
        agent,
        &format!("{OPEN_API_BASE}/device/campaigns/list"),
        &h,
        &json!({}),
    );
    if status != 200 {
        return Vec::new();
    }
    let Some(b) = body else { return Vec::new() };
    dig(&b, &["campaigns", "items"])
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = dig(c, &["campaignId", "id"]).and_then(|v| v.as_str())?;
                    let status_v = dig(c, &["status", "claimStatus"]).and_then(|v| v.as_str()).unwrap_or("");
                    let claimable = dig(c, &["claimable"]).and_then(|v| v.as_bool()).unwrap_or(false);
                    (claimable || status_v.eq_ignore_ascii_case("claimable"))
                        .then(|| (id.to_string(), dig(c, &["type", "campaignType"]).and_then(|v| v.as_str()).unwrap_or("").to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Claim one campaign → (ok, message).
fn claim_campaign(agent: &ureq::Agent, cfg: &AppConfig, creds: &QoderCreds, id: &str) -> (bool, String) {
    let h = build_auth_headers(cfg, creds);
    let (status, body, raw) = http::post_json(
        agent,
        &format!("{OPEN_API_BASE}/device/campaigns/claim"),
        &h,
        &json!({ "campaignId": id, "requestId": random_hex(16) }),
    );
    if status == 0 {
        return (false, "网络不可达".into());
    }
    let code_ok = body
        .as_ref()
        .and_then(|b| dig(b, &["code"]).and_then(|v| v.as_i64()))
        .map(|c| c == 0)
        .unwrap_or((200..300).contains(&status));
    let msg = body
        .as_ref()
        .and_then(|b| dig(b, &["message", "msg", "errorMessage"]).and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| truncate(&raw, 120));
    // already-claimed tolerance (source treats repeat claims as already)
    let low = msg.to_ascii_lowercase();
    if code_ok || low.contains("already") || msg.contains("已领") || status == 503 && low.contains("repeat") {
        if low.contains("already") || msg.contains("已领") {
            return (true, "今日已领取".into());
        }
        return (true, "领取成功".into());
    }
    (false, msg)
}

/// One Qoder round: per account ensure fresh → list campaigns → claim each.
pub fn run_round(store: &Store, cfg: &AppConfig, sink: &mut dyn Sink) -> Value {
    let accounts_file = store.accounts();
    let accounts: Vec<Account> = accounts_file
        .accounts
        .iter()
        .filter(|a| a.app == APP_QODER && a.enabled)
        .cloned()
        .collect();
    let agent = http::agent(30);
    let mut ok = 0usize;
    let mut already = 0usize;
    let mut failed = 0usize;
    let mut file_mut = accounts_file.clone();

    sink.emit(&Event::start(accounts.len()));
    let now_ms = chrono::Utc::now().timestamp_millis();

    for (i, acc) in accounts.iter().enumerate() {
        if i > 0 && cfg.account_gap_secs > 0 {
            std::thread::sleep(std::time::Duration::from_secs(cfg.account_gap_secs));
        }
        let idx = i + 1;
        let aid = acc.user_id.clone();
        let name = if acc.name.is_empty() { aid.clone() } else { acc.name.clone() };
        let mut creds = QoderCreds::from_value(acc.qoder_creds.as_ref());
        if creds.access_token.is_empty() {
            failed += 1;
            sink.emit(&Event::account_fail(idx, &aid, &name, "无可用凭证", None, None, None));
            continue;
        }
        if need_refresh(&creds, now_ms) {
            let (new, rejected) = refresh_token_once_ex(&agent, cfg, &creds);
            match new {
                Some(n) => {
                    creds = n;
                    for a in file_mut.accounts.iter_mut() {
                        if a.app == APP_QODER && a.user_id == aid {
                            a.qoder_creds = Some(creds.to_value());
                        }
                    }
                    let _ = store.save_accounts(&file_mut);
                }
                None if rejected => {
                    failed += 1;
                    for a in file_mut.accounts.iter_mut() {
                        if a.app == APP_QODER && a.user_id == aid {
                            a.needs_relogin = true;
                            a.relogin_reason = "refresh 被服务端拒绝".into();
                        }
                    }
                    let _ = store.save_accounts(&file_mut);
                    sink.emit(&Event::account_fail(idx, &aid, &name, "凭证已失效，需重新授权", None, None, None));
                    continue;
                }
                None => {}
            }
        }
        if creds.machine_id.is_empty() {
            let profile = DeviceProfile::generate(&aid);
            creds.machine_id = profile.machine_code.clone();
            for a in file_mut.accounts.iter_mut() {
                if a.app == APP_QODER && a.user_id == aid {
                    a.qoder_device_profile = Some(serde_json::to_value(&profile).unwrap_or(Value::Null));
                    a.qoder_creds = Some(creds.to_value());
                }
            }
            let _ = store.save_accounts(&file_mut);
        }
        let campaigns = fetch_campaigns(&agent, cfg, &creds);
        if campaigns.is_empty() {
            // unknown status: not a hard failure (matches failed_empty_campaigns semantics)
            sink.emit(&Event::account_already(idx, &aid, &name, None));
            already += 1;
            continue;
        }
        let mut any_ok = false;
        let mut any_already = true;
        let mut last_msg = String::new();
        for (cid, _kind) in &campaigns {
            let (okc, msg) = claim_campaign(&agent, cfg, &creds, cid);
            last_msg = msg;
            if okc {
                any_ok = true;
                if last_msg.contains("已领") {
                    // stays already
                } else {
                    any_already = false;
                }
            } else {
                any_already = false;
            }
        }
        if any_ok && !any_already {
            ok += 1;
            sink.emit(&Event::account_success(idx, &aid, &name, Some(0), &last_msg, None, None));
        } else if any_already {
            already += 1;
            sink.emit(&Event::account_already(idx, &aid, &name, None));
        } else {
            failed += 1;
            sink.emit(&Event::account_fail(idx, &aid, &name, &last_msg, None, None, None));
        }
    }

    let _ = store.save_accounts(&file_mut);
    let now_txt = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    store.append_checkin_log(&format!(
        "[{now_txt}] [qoder] 成功{ok}/已领{already}/失败{failed}/总计{}",
        accounts.len()
    ));
    sink.emit(&Event::done(ok, already, failed));
    Event::done(ok, already, failed).to_json()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_profile_shapes_and_stability() {
        let p1 = DeviceProfile::generate("uid-1");
        let p2 = DeviceProfile::generate("uid-1");
        let q = DeviceProfile::generate("uid-2");
        assert_eq!(p1.machine_code.len(), 18);
        assert_eq!(p1.machine_type.len(), 17);
        assert!(p1.hostname.starts_with("PC-"));
        assert_eq!(p1, p2, "same uid → same profile forever");
        assert_ne!(p1.machine_code, q.machine_code);
    }

    #[test]
    fn headers_carry_cosy_set() {
        let mut creds = QoderCreds::default();
        creds.access_token = "tok".into();
        creds.machine_id = "0123456789abcdef0123456789".into();
        let cfg = AppConfig::default();
        let h = build_auth_headers(&cfg, &creds);
        let get = |k: &str| h.iter().find(|(hk, _)| hk == k).map(|(_, v)| v.clone());
        assert_eq!(get("Authorization").unwrap(), "Bearer tok");
        assert_eq!(get("Cosy-ClientType").unwrap(), "10");
        assert_eq!(get("User-Agent").unwrap(), "Qoder");
        assert_eq!(get("Cosy-MachineId").unwrap(), "0123456789abcdef0123456789");
        assert_eq!(get("Cosy-MachineCode").unwrap(), "0123456789abcdef01"); // 18 chars
        assert_eq!(get("Cosy-MachineType").unwrap(), "0123456789abcdef0"); // 17 chars
        assert_eq!(get("Cosy-MachineOS").unwrap(), CLIENT_MACHINE_OS_ANDROID);
        // windows flavor restores desktop OS tag
        let mut cfg_w = AppConfig::default();
        cfg_w.platform_flavor = "windows".into();
        let h2 = build_auth_headers(&cfg_w, &creds);
        assert_eq!(
            h2.iter().find(|(k, _)| k == "Cosy-MachineOS").unwrap().1,
            CLIENT_MACHINE_OS_DESKTOP
        );
    }

    #[test]
    fn poll_success_maps_captured_shape() {
        // R-10 capture sample (tokens redacted), source-locked shape
        let body: Value = serde_json::from_str(
            r#"{"id":"01a0e03b-5f90-73b8-958e-443488ccf81e","token":"dt-placeholder","user_id":"01a0dff8-cc47-7b64-bd6d-d9cb2aea6792","code_challenge":"kiGczYiWCRH2Csra26Fx4NlhHp2n2CLT8QhMD678dqQ","code_challenge_method":"S256","nonce":"8872dcb1-d9c6-4453-bfb6-3bcbe9b0500b","expires_at":"2026-10-27T00:19:42Z","refresh_token_id":"01a0e03b-5f8f-7e4c-b112-1de3f25908cd","created_at":"2026-09-27T00:19:42Z","updated_at":"2026-09-27T00:19:42Z","refresh_token":"rt-placeholder","expires_in":2591999999,"refresh_token_expires_in":31103999999,"refresh_token_expires_at":"2027-09-22T00:19:42Z"}"#,
        )
        .unwrap();
        let (creds, uid) =
            parse_poll_success(&body, "8872dcb1-d9c6-4453-bfb6-3bcbe9b0500b").unwrap();
        assert_eq!(creds.access_token, "dt-placeholder");
        assert_eq!(creds.kind, "client");
        assert_eq!(uid, "01a0dff8-cc47-7b64-bd6d-d9cb2aea6792");
        // ≈30 天（毫秒级原样归一）
        let now = chrono::Utc::now().timestamp_millis();
        let days = (creds.expires_at_ms.unwrap() - now) as f64 / 86_400_000.0;
        assert!((29.0..=31.0).contains(&days));
        // refresh_token_expires_in=31103999999 ms ≈360 天
        let rt_days = (creds.refresh_expires_at_ms.unwrap() - now) as f64 / 86_400_000.0;
        assert!((350.0..=370.0).contains(&rt_days));
    }

    #[test]
    fn poll_success_rejects_pending_and_nonce_mismatch() {
        // pending shape (404 body)
        let pending: Value =
            serde_json::from_str(r#"{"errorCode":"NotFound","errorMessage":"Not found"}"#).unwrap();
        assert!(parse_poll_success(&pending, "n").is_none());
        // nonce mismatch must be rejected (session-confusion guard)
        let body = json!({"token":"dt-x","user_id":"u1","nonce":"session-a","expires_in":2591999999i64});
        assert!(parse_poll_success(&body, "session-a").is_some());
        assert!(parse_poll_success(&body, "session-b").is_none());
    }

    #[test]
    fn device_flow_shape_matches_source() {
        let f1 = DeviceFlow::new();
        let f2 = DeviceFlow::new();
        assert_ne!(f1.nonce, f2.nonce);
        assert_ne!(f1.verifier, f2.verifier);
        assert_eq!(f1.verifier.len(), 64);
        assert!(f1.auth_url.starts_with(DEVICE_AUTH_BASE));
        for key in ["challenge=", "challenge_method=S256", "nonce=", "machine_id=", "client_id="] {
            assert!(f1.auth_url.contains(key), "auth_url 缺少 {key}");
        }
        assert!(f1.auth_url.contains(&format!("client_id={DEVICE_AUTH_CLIENT_ID}")));
        // 不携带 directLogin：官方客户端与社区实现均不发送该参数
        assert!(!f1.auth_url.contains("directLogin"));
        // challenge 可由 verifier 复算（S256 绑定）
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(f1.verifier.as_bytes()));
        assert!(f1.auth_url.contains(&format!("challenge={expect}")));
        // compat 模式不带 client_id
        let fc = DeviceFlow::new_compat();
        assert!(fc.client_id.is_none());
        assert!(!fc.auth_url.contains("client_id="));
    }

    #[test]
    fn normalize_expires_in_ms_and_seconds() {
        // 阈值边界：30 天秒级恰为分界（>2_592_000 判毫秒原样），其下判秒级 ×1000
        assert_eq!(normalize_expires_in(2_592_000), 2_592_000_000);
        assert_eq!(normalize_expires_in(2_592_001), 2_592_001);
        assert_eq!(normalize_expires_in(3600), 3_600_000);
        assert_eq!(normalize_expires_in(2591999999), 2591999999);
    }

    #[test]
    fn need_refresh_gate() {
        let now = 1_700_000_000_000i64;
        let mut c = QoderCreds::default();
        c.expires_at_ms = Some(now + 25 * 3_600_000);
        assert!(!need_refresh(&c, now));
        c.expires_at_ms = Some(now + 1 * 3_600_000);
        assert!(need_refresh(&c, now));
        c.expires_at_ms = None;
        assert!(need_refresh(&c, now));
    }
}
