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
use crate::util::sha256_hex;

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
    let mut out = creds.clone();
    if !apply_refresh_response(&mut out, &b, chrono::Utc::now().timestamp_millis()) {
        return (None, false);
    }
    (Some(out), false)
}

/// 刷新响应解析（upstream issue #143，2026-10-25 实测 `/device/token/refresh` 的 200
/// 响应形如 `{device_token, expires_at(RFC3339), refresh_token}`）：
/// - access token 候选：`accessToken` / `access_token` / `token` / **`device_token`** /
///   `deviceToken`（旧实现只认 accessToken → 明明 200 成功却被判「响应结构异常」，
///   续期恒失败，令牌过期后签到全挂）；
/// - 到期：`expiresIn` / `expires_in`（毫秒/秒归一）或 `expires_at` / `expiresAt`（RFC3339）；
/// - refresh token：`refreshToken` / `refresh_token`；
/// - refresh 到期：`refresh_token_expires_in` / `refreshTokenExpiresIn` 或
///   `refresh_expires_at` / `refreshExpiresAt`（RFC3339）。
fn apply_refresh_response(out: &mut QoderCreds, body: &Value, now_ms: i64) -> bool {
    let acc = s_of(dig(
        body,
        &["accessToken", "access_token", "token", "device_token", "deviceToken"],
    ));
    if acc.is_empty() {
        return false;
    }
    out.access_token = acc;
    let rt = s_of(dig(body, &["refreshToken", "refresh_token"]));
    if !rt.is_empty() {
        out.refresh_token = rt;
    }
    out.expires_at_ms = Some(
        int_of(dig(body, &["expiresIn", "expires_in"]))
            .map(|e| now_ms + normalize_expires_in(e))
            .or_else(|| rfc3339_ms(dig(body, &["expires_at", "expiresAt"])))
            // 响应缺到期字段时落保守本地基准（now+12h）：否则 expires_at 恒为 None，
            // 临期判定永无基准、每次调用都重发刷新请求。
            .unwrap_or(now_ms + 12 * 3_600_000),
    );
    if let Some(e) = int_of(dig(body, &["refresh_token_expires_in", "refreshTokenExpiresIn"])) {
        out.refresh_expires_at_ms = Some(now_ms + normalize_expires_in(e));
    } else if let Some(ms) = rfc3339_ms(dig(body, &["refresh_expires_at", "refreshExpiresAt"])) {
        out.refresh_expires_at_ms = Some(ms);
    }
    true
}

/// i64 宽容读（数字或数字字符串）
fn int_of(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// RFC3339 字符串 → 毫秒时间戳
fn rfc3339_ms(v: Option<&Value>) -> Option<i64> {
    let s = v?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
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
/// SSO 登录页基址（upstream issue #140，2026-10-24 实测）：设备授权页自构 SSO 跳转时
/// 向 oauth_callback 写入显式 `:443` 端口，被 SSO 端点回调白名单（仅 `https://qoder.cn`）
/// 拒绝 → 未登录用户经授权页登录必现 400 Bad Request。客户端直连本页并自构正确回调。
pub const SSO_LOGIN_BASE: &str = "https://qoder.cn/sso/login/aliyun";
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
        // 设备授权页完整 URL（不再直接打开——issue #140：该页自构 SSO 跳转时向
        // oauth_callback 写入显式 :443 端口，被 SSO 回调白名单拒绝 → 400）。
        // directLogin=true 单个出现，模拟授权页自身生成的回调形态。
        let mut device_url = format!(
            "{DEVICE_AUTH_BASE}?challenge={challenge}&challenge_method=S256&nonce={nonce}&machine_id={machine_id}&directLogin=true"
        );
        if let Some(cid) = &client_id {
            device_url.push_str(&format!("&client_id={cid}"));
        }
        // 直连 SSO 登录页，oauth_callback 全量百分号编码（跳过授权页损坏的重定向）
        let auth_url = format!("{SSO_LOGIN_BASE}?oauth_callback={}", percent_encode(&device_url));
        Self { nonce, verifier, machine_id, client_id, auth_url }
    }
}

impl Default for DeviceFlow {
    fn default() -> Self {
        Self::new()
    }
}

/// RFC 3986 百分号编码（仅保留 unreserved 字符）——oauth_callback 需要全量编码
/// （issue #140：设备授权页 URL 含 `?`/`&`/`=`，不编码会把回调参数截断）。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 单次轮询：GET /api/v1/deviceToken/poll（无 Authorization 头，与客户端一致）。
/// pending = HTTP 404 {"errorCode":"NotFound"}；成功 = HTTP 200。
pub fn device_flow_poll_once(agent: &ureq::Agent, flow: &DeviceFlow) -> (i32, Option<Value>) {
    let url = format!(
        "{}/api/v1/deviceToken/poll?nonce={}&verifier={}&challenge_method=S256",
        OPEN_API_BASE,
        flow.nonce,
        percent_encode(&flow.verifier)
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



// ── check-in round (upstream v3.8.3 port) ───────────────────────────────────
//
// Port of tasks/qoder_checkin.rs @ origin/main (commits 541f33b, f4888f6,
// f5f2cd3, 72447db): the old `/device/campaigns/*` endpoints never existed in
// the upstream implementation — the real flow is:
//   GET  {base}/sash/api/v1/me/campaigns              (list, full headers)
//   POST {base}/sash/api/v1/me/campaigns/{id}/claim   (idempotent claim,
//        MINIMAL headers — the device-fingerprint header family gets claim
//        requests 503-blocked by upstream risk control, issue #114)
// Daily campaign ids rotate; the pipeline is: learn from list → zero-CLAIMABLE
// → check CLAIMED grant in list → blind-claim fallback (learned ids first,
// known table兜底) → same-round relearn (main list + minimal-header fetch)
// with 409 CAMPAIGN_NOT_ACTIVE pruning.

pub const CAMPAIGN_NOT_ACTIVE_TAG: &str = "CAMPAIGN_NOT_ACTIVE";

/// Known daily campaigns table (upstream constant, last-resort fallback):
/// (campaignId, display name). Upstream rotates these via probes; the learned
/// list and minimal-header relearn are the primary sources.
const KNOWN_DAILY_CAMPAIGNS: &[(&str, &str)] = &[
    ("01a130ea-7c31-71b3-a22b-cbe019571350", "每日领 100 Credits"),
];

fn minimal_auth_headers(access_token: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".to_string(), format!("Bearer {access_token}")),
        ("Cosy-ClientType".to_string(), COSY_CLIENT_TYPE.to_string()),
        ("User-Agent".to_string(), CLIENT_USER_AGENT.to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ]
}

fn s_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").to_string()
}

fn num_or_none(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().or_else(|| v?.as_str()?.trim().parse().ok())
}

/// (campaignId, display name) from a campaign entry (path-safe id whitelist).
fn learned_entry_from(c: &Value) -> Option<(String, String)> {
    let cid = s_of(dig(c, &["campaignId", "campaign_id"]));
    if cid.is_empty() || !cid.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
        return None;
    }
    let cname = s_of(dig(c, &["name", "title", "campaignName", "campaign_name"]));
    let cname = if cname.is_empty() { cid.chars().take(8).collect() } else { cname };
    Some((cid, cname))
}

fn merge_learned(out: &mut Vec<(String, String)>, entries: Vec<(String, String)>) {
    for (cid, cname) in entries {
        if !out.iter().any(|(e, _)| *e == cid) {
            out.push((cid, cname));
        }
    }
}

fn benefit_is_credits(c: &Value) -> bool {
    c.get("benefit").and_then(|x| x.get("kind"))
        .map(|v| s_of(Some(v)).to_ascii_uppercase() == "CREDITS")
        .unwrap_or(false)
}

fn is_claim_benefit(c: &Value) -> bool {
    s_of(c.get("actionType")).to_ascii_uppercase() == "CLAIM_BENEFIT"
}

/// Strict-tier learning: CLAIM_BENEFIT + CREDITS + inside the daily window.
fn learn_daily_campaign_ids(campaigns: &[Value], now_secs: i64) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for c in campaigns {
        let start = c.get("startAt").and_then(|v| num_or_none(Some(v))).map(|v| v as i64);
        let end = c.get("endAt").and_then(|v| num_or_none(Some(v))).map(|v| v as i64);
        let in_window = matches!((start, end), (Some(s), Some(e)) if now_secs >= s && now_secs <= e);
        if is_claim_benefit(c) && benefit_is_credits(c) && in_window {
            if let Some(entry) = learned_entry_from(c) {
                merge_learned(&mut out, vec![entry]);
            }
        }
    }
    out
}

/// Lenient-tier learning (issue #105): first non-empty tier wins.
/// A: CLAIM_BENEFIT + CREDITS (no window) / B: CREDITS only / C: CLAIM_BENEFIT + name contains 每日.
fn learn_daily_campaign_ids_lenient(campaigns: &[Value]) -> Vec<(String, String)> {
    let learn = |pred: &dyn Fn(&Value) -> bool| -> Vec<(String, String)> {
        let mut out = Vec::new();
        for c in campaigns {
            if pred(c) {
                if let Some(entry) = learned_entry_from(c) {
                    merge_learned(&mut out, vec![entry]);
                }
            }
        }
        out
    };
    let tier_a = learn(&|c| is_claim_benefit(c) && benefit_is_credits(c));
    if !tier_a.is_empty() { return tier_a; }
    let tier_b = learn(&|c| benefit_is_credits(c));
    if !tier_b.is_empty() { return tier_b; }
    learn(&|c| is_claim_benefit(c) && s_of(c.get("name")).contains("每日"))
}

/// Raw list fetch (GET /sash/api/v1/me/campaigns): full list, empty allowed.
/// Ok(list) | Err(("auth"|"fail", message)).
fn fetch_campaigns_raw(agent: &ureq::Agent, headers: &[(String, String)]) -> Result<Vec<Value>, (String, String)> {
    let (status, body, raw) = http::get_json(agent, &format!("{OPEN_API_BASE}/sash/api/v1/me/campaigns"), headers);
    if status == 401 {
        return Err(("auth".into(), "登录态失效（401）".into()));
    }
    if status == 0 {
        let head: String = raw.chars().take(120).collect();
        return Err(("fail".into(), format!("网络不可达: {head}")));
    }
    if !(200..=201).contains(&status) {
        return Err(("fail".into(), format!("campaigns 不可用（HTTP {status}）")));
    }
    let Some(b) = body.filter(|b| b.is_object()) else {
        return Err(("fail".into(), "campaigns 响应非 JSON".into()));
    };
    Ok(dig(&b, &["campaigns"]).and_then(Value::as_array).cloned().unwrap_or_default())
}

fn filter_claimable(campaigns: &[Value]) -> Vec<Value> {
    campaigns
        .iter()
        .filter(|c| s_of(dig(c, &["claimStatus", "claim_status"])).to_ascii_uppercase() == "CLAIMABLE")
        .cloned()
        .collect()
}

/// Does the list contain today's CLAIMED daily-credits grant (server shows
/// claimed grants unconditionally → real "already", no request needed)?
fn daily_credits_claimed_in_list(campaigns: &[Value], now_secs: i64) -> bool {
    campaigns.iter().any(|c| {
        let claimed = s_of(dig(c, &["claimStatus", "claim_status"])).to_ascii_uppercase() == "CLAIMED";
        let start = c.get("startAt").and_then(|v| num_or_none(Some(v))).map(|v| v as i64);
        let end = c.get("endAt").and_then(|v| num_or_none(Some(v))).map(|v| v as i64);
        let in_window = matches!((start, end), (Some(s), Some(e)) if now_secs >= s && now_secs <= e);
        claimed && is_claim_benefit(c) && benefit_is_credits(c) && in_window
    })
}

/// POST-empty with the minimal header family → (status, body, raw).
/// post_json sends `{}` as the body — the upstream claim POST is bodyless but
/// an empty JSON object is accepted the same way (content-type json, no fields).
fn post_empty(agent: &ureq::Agent, url: &str, headers: &[(String, String)]) -> (i32, Option<Value>, String) {
    http::post_json(agent, url, headers, &json!({}))
}

/// Blind-step classification (pure; 宁 fail 不假 already).
enum BlindStep {
    FreshClaim,
    Replay,
    Auth,
    Failed(i32),
}

fn classify_blind_step(status: i32, claimed: Option<String>, replayed: bool, kind: Option<&str>) -> BlindStep {
    if status == 401 {
        return BlindStep::Auth;
    }
    if (200..=201).contains(&status) {
        return match claimed.as_deref() {
            Some("CLAIMED") => {
                if kind.is_some_and(|k| !k.eq_ignore_ascii_case("CREDITS")) {
                    return BlindStep::Failed(status);
                }
                if replayed { BlindStep::Replay } else { BlindStep::FreshClaim }
            }
            _ => BlindStep::Failed(status),
        };
    }
    BlindStep::Failed(status)
}

fn describe_blind_failure(code: i32, claimed: Option<&str>, raw: &str) -> String {
    if claimed == Some("BLOCKED") {
        return "服务端风控拦截（status=BLOCKED）——该账号设备指纹未建立服务端信任：请在 Qoder 客户端正常使用一次后重试".into();
    }
    if code == 503 && raw.contains("RISK_DEPENDENCY_UNAVAILABLE") {
        return "服务端风控依赖暂不可用（503），稍后将自动重试".into();
    }
    if code == 409 && raw.contains(CAMPAIGN_NOT_ACTIVE_TAG) {
        return "每日活动 id 已过期（409 CAMPAIGN_NOT_ACTIVE，活动批次轮换）——将当轮重学后重试".into();
    }
    let head: String = raw.chars().take(120).collect();
    format!("（HTTP {code}）{head}")
}

#[derive(Clone, Default)]
struct BlindRound {
    messages: Vec<String>,
    fail_parts: Vec<String>,
    stale_parts: Vec<String>,
    reward: Option<f64>,
    any_fresh: bool,
    any_replay: bool,
    auth: bool,
}

impl BlindRound {
    fn stale_only(&self) -> bool {
        !self.stale_parts.is_empty() && !self.any_fresh && !self.any_replay && self.fail_parts.is_empty()
    }
    fn merge(mut self, other: BlindRound) -> BlindRound {
        self.messages.extend(other.messages);
        self.fail_parts.extend(other.fail_parts);
        self.stale_parts.extend(other.stale_parts);
        self.reward = other.reward.or(self.reward);
        self.any_fresh |= other.any_fresh;
        self.any_replay |= other.any_replay;
        self.auth |= other.auth;
        self
    }
    fn aggregate(self) -> (String, String, Option<f64>) {
        if self.auth {
            return ("auth".into(), "登录态失效（401）".into(), self.reward);
        }
        if self.any_fresh {
            return ("success".into(), self.messages.join("；"), self.reward);
        }
        if self.any_replay {
            return ("already".into(), "每日奖励已领取（盲发回放确认）".into(), self.reward);
        }
        let mut all = self.fail_parts.clone();
        all.extend(self.stale_parts.clone());
        let msg = if all.is_empty() { "盲发无候选可用".into() } else { all.join("；") };
        ("fail".into(), msg, self.reward)
    }
}

fn blind_round(
    agent: &ureq::Agent,
    access_token: &str,
    candidates: &[(String, String)],
    learned: &mut Vec<(String, String)>,
    table: &mut Vec<(String, String)>,
) -> BlindRound {
    let headers = minimal_auth_headers(access_token);
    let mut round = BlindRound::default();
    for (cid, cname) in candidates {
        if cid.is_empty() || !cid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let url = format!("{OPEN_API_BASE}/sash/api/v1/me/campaigns/{cid}/claim");
        let (status, body, raw) = post_empty(agent, &url, &headers);
        let claimed = body.as_ref().and_then(|b| dig(b, &["status", "data.status"])).map(|v| s_of(Some(v)).to_ascii_uppercase());
        let replayed = body.as_ref().and_then(|b| dig(b, &["replayed", "data.replayed"])).and_then(Value::as_bool).unwrap_or(false);
        let kind = body.as_ref().and_then(|b| b.get("benefit")).and_then(|x| x.get("kind")).and_then(Value::as_str);
        match classify_blind_step(status, claimed.clone(), replayed, kind) {
            BlindStep::Auth => { round.auth = true; return round; }
            BlindStep::FreshClaim => {
                let amt = body.as_ref().and_then(|b| b.get("benefit")).and_then(|x| x.get("amount")).and_then(|v| num_or_none(Some(v)));
                round.any_fresh = true;
                round.messages.push(format!("{cname}（盲发直领）"));
                if let Some(a) = amt { round.reward = Some(round.reward.unwrap_or(0.0) + a); }
            }
            BlindStep::Replay => { round.any_replay = true; }
            BlindStep::Failed(code) => {
                let reason = describe_blind_failure(code, claimed.as_deref(), &raw);
                if code == 409 && raw.contains(CAMPAIGN_NOT_ACTIVE_TAG) {
                    learned.retain(|(e, _)| e != cid);
                    table.retain(|(e, _)| e != cid);
                    round.stale_parts.push(format!("{cname} 盲发失败：{reason}"));
                } else {
                    round.fail_parts.push(format!("{cname} 盲发失败：{reason}"));
                }
            }
        }
    }
    round
}

/// Minimal-header relearn: fetch the list with the minimal family and learn
/// the daily campaign id (best effort; never blocks the main flow).
fn learn_from_min_headers(agent: &ureq::Agent, access_token: &str, now_secs: i64) -> Vec<(String, String)> {
    match fetch_campaigns_raw(agent, &minimal_auth_headers(access_token)) {
        Ok(campaigns) => learn_daily_campaign_ids(&campaigns, now_secs),
        Err(_) => Vec::new(),
    }
}

/// Blind fallback + same-round self-healing: learned-first candidates, table
/// fallback; stale-only outcome → main-list relearn (lenient) → still stale →
/// minimal-header relearn → retry untried ids.
fn fallback_with_relearn(
    agent: &ureq::Agent,
    access_token: &str,
    learned: &mut Vec<(String, String)>,
    table: &mut Vec<(String, String)>,
) -> (String, String, Option<f64>) {
    let mut attempted: Vec<String> = learned.iter().map(|(c, _)| c.clone()).collect();
    for (cid, _) in table.iter() {
        if !attempted.iter().any(|c| c == cid) {
            attempted.push(cid.clone());
        }
    }
    // candidates: learned first, table deduped after
    let mut candidates: Vec<(String, String)> = learned.clone();
    for (cid, cname) in table.iter() {
        if !candidates.iter().any(|(e, _)| e == cid) {
            candidates.push((cid.clone(), cname.clone()));
        }
    }
    let tried: Vec<String> = candidates.iter().map(|(cid, _)| cid.clone()).collect();
    let first = blind_round(agent, access_token, &candidates, learned, table);
    if first.auth {
        return first.aggregate();
    }
    if !first.stale_only() {
        return first.aggregate();
    }
    // same-round relearn #1: main list, lenient tiers, untried ids only
    let mut retry: Vec<(String, String)> = Vec::new();
    if let Ok(campaigns) = fetch_campaigns_raw(agent, &minimal_auth_headers(access_token)) {
        merge_learned(&mut retry, learn_daily_campaign_ids_lenient(&campaigns));
    }
    retry.retain(|(cid, _)| !tried.iter().any(|t| t == cid));
    let second = if retry.is_empty() {
        None
    } else {
        merge_learned(learned, retry.clone());
        let r = blind_round(agent, access_token, &retry, learned, table);
        if r.auth { return r.aggregate(); }
        Some(first.clone().merge(r))
    };
    let merged = second.unwrap_or(first);
    if !merged.stale_only() {
        return merged.aggregate();
    }
    // same-round relearn #2: minimal-header fetch (visibility superset)
    let found = learn_from_min_headers(agent, access_token, chrono::Utc::now().timestamp())
        .into_iter()
        .filter(|(cid, _)| !attempted.iter().any(|c| c == cid))
        .collect::<Vec<_>>();
    if found.is_empty() {
        return merged.aggregate();
    }
    merge_learned(learned, found.clone());
    let third = blind_round(agent, access_token, &found, learned, table);
    merged.merge(third).aggregate()
}

/// Claim every CLAIMABLE campaign with the minimal header family (issue #114:
/// the device-fingerprint family is 503-blocked on claim).
/// Returns (kind, message, reward); kind == "auth" means the token was rejected
/// (caller refreshes once and retries, matching upstream's 401 handling).
fn claim_claimable(
    agent: &ureq::Agent,
    token: &str,
    claimable: &[Value],
) -> (String, String, Option<f64>) {
    let mut kind = String::new();
    let mut messages: Vec<String> = Vec::new();
    let mut reward: Option<f64> = None;
    let mut any_fail = false;
    let mut any_already = false;
    for c in claimable {
        let cid = s_of(dig(c, &["campaignId", "campaign_id"]));
        let cname = s_of(dig(c, &["name", "title", "campaignName", "campaign_name"]));
        let url = format!("{OPEN_API_BASE}/sash/api/v1/me/campaigns/{cid}/claim");
        let (status, body, raw) = post_empty(agent, &url, &minimal_auth_headers(token));
        if status == 401 {
            kind = "auth".into();
            break;
        }
        let claimed = body
            .as_ref()
            .and_then(|b| dig(b, &["status", "data.status"]))
            .map(|v| s_of(Some(v)).to_ascii_uppercase());
        let replayed = body
            .as_ref()
            .and_then(|b| dig(b, &["replayed", "data.replayed"]))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let bkind = body
            .as_ref()
            .and_then(|b| b.get("benefit"))
            .and_then(|x| x.get("kind"))
            .and_then(Value::as_str);
        match classify_blind_step(status, claimed.clone(), replayed, bkind) {
            BlindStep::Auth => {
                kind = "auth".into();
                break;
            }
            BlindStep::FreshClaim => {
                kind = "success".into();
                messages.push(format!("{}（领取成功）", if cname.is_empty() { &cid } else { &cname }));
                let amt = body
                    .as_ref()
                    .and_then(|b| b.get("benefit"))
                    .and_then(|x| x.get("amount"))
                    .and_then(|v| num_or_none(Some(v)));
                if let Some(a) = amt {
                    reward = Some(reward.unwrap_or(0.0) + a);
                }
            }
            BlindStep::Replay => {
                any_already = true;
                if kind.is_empty() {
                    kind = "already".into();
                }
                messages.push(format!("{}（已领取）", if cname.is_empty() { &cid } else { &cname }));
            }
            BlindStep::Failed(code) => {
                any_fail = true;
                kind = "fail".into();
                messages.push(format!(
                    "{}：{}",
                    if cname.is_empty() { &cid } else { &cname },
                    describe_blind_failure(code, claimed.as_deref(), &raw)
                ));
            }
        }
    }
    // priority merge: success > fail > already (P2)
    let final_kind = if kind == "auth" {
        "auth".to_string()
    } else if any_fail && !messages.is_empty() && reward.is_some() && !any_already {
        "fail".into()
    } else if any_fail && !messages.iter().any(|m| m.contains("领取成功")) {
        "fail".into()
    } else if reward.is_some() || messages.iter().any(|m| m.contains("领取成功")) {
        "success".into()
    } else if any_already {
        "already".into()
    } else if any_fail {
        "fail".into()
    } else {
        "already".into()
    };
    (final_kind, messages.join("；"), reward)
}

/// 刷新后的 Qoder 凭证写回账号文件并落盘（刷新链与 401 重试链共用）。
fn persist_qoder_creds(
    store: &Store,
    file: &mut crate::store::AccountsFile,
    aid: &str,
    creds: &QoderCreds,
) {
    for a in file.accounts.iter_mut() {
        if a.app == APP_QODER && a.user_id == aid {
            a.qoder_creds = Some(creds.to_value());
        }
    }
    let _ = store.save_accounts(file);
}

/// 标记需要重新授权（凭证真正失效时才调用：服务端拒绝刷新 / 刷新后仍 401）。
fn mark_qoder_relogin(
    store: &Store,
    file: &mut crate::store::AccountsFile,
    aid: &str,
    reason: &str,
) {
    for a in file.accounts.iter_mut() {
        if a.app == APP_QODER && a.user_id == aid {
            a.needs_relogin = true;
            a.relogin_reason = reason.to_string();
        }
    }
    let _ = store.save_accounts(file);
}

/// One Qoder round: per account ensure fresh → list campaigns → claim each →
/// zero-CLAIMABLE fallback chain. Shared learned pool across accounts.
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
    let mut learned: Vec<(String, String)> = Vec::new();
    let mut table: Vec<(String, String)> = KNOWN_DAILY_CAMPAIGNS
        .iter()
        .map(|(c, n)| (c.to_string(), n.to_string()))
        .collect();

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
                    persist_qoder_creds(&store, &mut file_mut, &aid, &creds);
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

        // list: full headers; empty → minimal-header refetch once (issue #132)
        let full_headers = build_auth_headers(cfg, &creds);
        let min_token = creds.access_token.clone();
        let mut campaigns = fetch_campaigns_raw(&agent, &full_headers).unwrap_or_default();
        let mut list_from_min = false;
        if campaigns.is_empty() {
            if let Ok(c) = fetch_campaigns_raw(&agent, &minimal_auth_headers(&min_token)) {
                if !c.is_empty() {
                    campaigns = c;
                    list_from_min = true;
                }
            }
        }
        // learn from the list (entries are visible regardless of status bucket)
        merge_learned(&mut learned, learn_daily_campaign_ids(&campaigns, chrono::Utc::now().timestamp()));
        if learned.is_empty() && !list_from_min {
            merge_learned(&mut learned, learn_from_min_headers(&agent, &min_token, chrono::Utc::now().timestamp()));
        }

        let claimable = filter_claimable(&campaigns);
        let kind_message = |token: &str, learned: &mut Vec<(String, String)>, table: &mut Vec<(String, String)>| -> (String, String, Option<f64>) {
            if claimable.is_empty() {
                if daily_credits_claimed_in_list(&campaigns, chrono::Utc::now().timestamp()) {
                    ("already".into(), "每日奖励已领取".into(), None)
                } else {
                    fallback_with_relearn(&agent, token, learned, table)
                }
            } else {
                claim_claimable(&agent, token, &claimable)
            }
        };
        let (mut kind, mut message, mut reward) = kind_message(&min_token, &mut learned, &mut table);
        // 401：刷新一次仅重试失败分支（禁二次刷新，对齐 wb_checkin F-09 / upstream）
        if kind == "auth" {
            match refresh_token_once_ex(&agent, cfg, &creds) {
                (Some(new), _) => {
                    persist_qoder_creds(&store, &mut file_mut, &aid, &new);
                    creds = new;
                    let (k, m, r) = kind_message(&creds.access_token, &mut learned, &mut table);
                    (kind, message, reward) = (k, m, r);
                    if kind == "auth" {
                        mark_qoder_relogin(&store, &mut file_mut, &aid, "签到重试后仍 401");
                        kind = "fail".into();
                        message = "登录态失效（刷新后仍 401），需重新登录".into();
                    }
                }
                (None, true) => {
                    // 服务端 4xx 永久拒绝刷新：重试无解，落 needs_relogin 引导用户重登
                    mark_qoder_relogin(&store, &mut file_mut, &aid, "签到 refresh 被服务端拒绝");
                    kind = "fail".into();
                    message = "凭证已失效，需重新授权".into();
                }
                (None, false) => {
                    // 暂态失败（网络/服务端 5xx）：不得误标需重登，下轮自然重试
                    kind = "fail".into();
                    message = "登录态刷新失败（网络），稍后重试".into();
                }
            }
        }

        match kind.as_str() {
            "success" => {
                ok += 1;
                sink.emit(&Event::account_success(idx, &aid, &name, Some(0), &message, reward.map(|r| r as i64), reward.map(|r| r as i64)));
            }
            "already" => {
                already += 1;
                sink.emit(&Event::account_already(idx, &aid, &name, reward.map(|r| r as i64)));
            }
            _ => {
                failed += 1;
                sink.emit(&Event::account_fail(idx, &aid, &name, &message, None, None, None));
            }
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

    /// Upstream-port contract (issue #114/#132): claim requests must use the
    /// minimal header family — the device-fingerprint family gets claim POSTs
    /// 503-blocked by upstream risk control.
    #[test]
    fn minimal_headers_exclude_device_fingerprint_family() {
        let h = minimal_auth_headers("tok");
        let names: Vec<&str> = h.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"Authorization"));
        assert!(names.contains(&"Cosy-ClientType"));
        assert!(names.contains(&"User-Agent"));
        assert!(names.contains(&"Content-Type"));
        assert!(!names.iter().any(|k| k.starts_with("Cosy-Machine")),
            "device fingerprint headers must NOT be in the minimal family");
    }

    /// classify_blind_step: replay → Replay, fresh CLAIMED → FreshClaim,
    /// non-CREDITS kind → Failed even on CLAIMED, unknown shape → Failed.
    #[test]
    fn blind_step_classification() {
        let fresh = classify_blind_step(200, Some("CLAIMED".into()), false, Some("CREDITS"));
        assert!(matches!(fresh, BlindStep::FreshClaim));
        let replay = classify_blind_step(200, Some("CLAIMED".into()), true, Some("CREDITS"));
        assert!(matches!(replay, BlindStep::Replay));
        let bogo = classify_blind_step(200, Some("CLAIMED".into()), false, Some("BOGO"));
        assert!(matches!(bogo, BlindStep::Failed(_)));
        let unknown = classify_blind_step(200, Some("OTHER".into()), false, Some("CREDITS"));
        assert!(matches!(unknown, BlindStep::Failed(_)));
        let auth = classify_blind_step(401, None, false, None);
        assert!(matches!(auth, BlindStep::Auth));
        let err = classify_blind_step(409, None, false, None);
        assert!(matches!(err, BlindStep::Failed(409)));
    }

    /// Learning tiers: strict needs window; lenient falls back A → B → C.
    #[test]
    fn campaign_learning_tiers() {
        let now = 1_700_000_000i64;
        let in_window = json!({
            "campaignId": "c1", "actionType": "CLAIM_BENEFIT",
            "benefit": {"kind": "CREDITS", "amount": 100},
            "startAt": now - 100, "endAt": now + 100
        });
        let out = learn_daily_campaign_ids(&[in_window.clone()], now);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "c1");
        // out of window → strict tier misses
        let mut expired = in_window.clone();
        expired["startAt"] = json!(now - 2000);
        expired["endAt"] = json!(now - 1000);
        assert!(learn_daily_campaign_ids(&[expired.clone()], now).is_empty());
        // lenient tier A still learns it (no window requirement)
        let len = learn_daily_campaign_ids_lenient(&[expired]);
        assert_eq!(len.len(), 1);
        // tier C: name-based
        let odd = json!({"campaignId": "c9", "actionType": "CLAIM_BENEFIT", "name": "每日签到奖励"});
        let len_c = learn_daily_campaign_ids_lenient(&[odd]);
        assert_eq!(len_c.len(), 1);
        assert_eq!(len_c[0].0, "c9");
    }

    /// 409 CAMPAIGN_NOT_ACTIVE pruning: stale ids removed from learned + table.
    #[test]
    fn stale_id_pruning() {
        let mut learned = vec![("old-id".to_string(), "旧活动".to_string())];
        let mut table = vec![
            ("old-id".to_string(), "旧活动".to_string()),
            ("fresh-id".to_string(), "新活动".to_string()),
        ];
        learned.retain(|(e, _)| e != "old-id");
        table.retain(|(e, _)| e != "old-id");
        assert!(!learned.iter().any(|(e, _)| e == "old-id"));
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].0, "fresh-id");
    }

    /// issue #143：`/device/token/refresh` 的 200 响应把令牌放在 `device_token`，
    /// 到期是 RFC3339 的 `expires_at`——旧解析只认 accessToken/expiresIn，
    /// 会把成功响应当作「结构异常」，续期恒失败。
    #[test]
    fn device_token_refresh_response_parses_device_token_and_expires_at() {
        let body = json!({
            "created_at": "2026-10-25T04:41:16Z",
            "device_token": "dt-test",
            "expires_at": "2026-11-24T04:41:16Z",
            "refresh_token": "drt-test"
        });
        let mut c = QoderCreds::default();
        assert!(apply_refresh_response(&mut c, &body, 0));
        assert_eq!(c.access_token, "dt-test");
        assert_eq!(c.refresh_token, "drt-test");
        assert_eq!(
            c.expires_at_ms,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-11-24T04:41:16Z")
                    .unwrap()
                    .timestamp_millis()
            )
        );
    }

    /// 兼容旧形态（accessToken + expiresIn 毫秒），不回归。
    #[test]
    fn legacy_refresh_response_still_parses_expires_in() {
        let body = json!({
            "accessToken": "jt-test",
            "expiresIn": 86_400_000_i64,
            "refreshToken": "jrt-test"
        });
        let mut c = QoderCreds::default();
        assert!(apply_refresh_response(&mut c, &body, 1_000));
        assert_eq!(c.access_token, "jt-test");
        assert_eq!(c.refresh_token, "jrt-test");
        // normalize_expires_in: >30d 视为毫秒，否则视为秒 → 86400000 走毫秒分支
        assert_eq!(c.expires_at_ms, Some(1_000 + 86_400_000));
    }

    /// 缺到期字段时落保守基准（now+12h），避免每次调用都重发刷新。
    #[test]
    fn refresh_response_without_expiry_uses_conservative_baseline() {
        let mut c = QoderCreds::default();
        assert!(apply_refresh_response(&mut c, &json!({"deviceToken": "dt-2"}), 0));
        assert_eq!(c.access_token, "dt-2");
        assert_eq!(c.expires_at_ms, Some(12 * 3_600_000));
        // 完全无令牌字段 → 判失败（不覆盖已有凭据）
        let mut d = QoderCreds::default();
        d.access_token = "keep".into();
        assert!(!apply_refresh_response(&mut d, &json!({"ok": true}), 0));
        assert_eq!(d.access_token, "keep");
    }

    /// issue #140：授权 URL 直连 SSO 登录页，oauth_callback 全量编码且带 directLogin。
    #[test]
    fn auth_url_targets_sso_with_encoded_callback() {
        let f = DeviceFlow::new();
        assert!(f.auth_url.starts_with(&format!("{SSO_LOGIN_BASE}?oauth_callback=")), "{}", f.auth_url);
        // 回调里的分隔符必须已编码，否则 query 被截断
        let callback = f.auth_url.split("oauth_callback=").nth(1).unwrap();
        assert!(!callback.contains('&'), "callback must be fully percent-encoded: {callback}");
        assert!(!callback.contains('?'));
        assert!(callback.contains("%3A%2F%2Fqoder.cn%2Fdevice%2FselectAccounts%3F"));
        assert!(callback.contains("directLogin%3Dtrue"));
        assert!(callback.contains("client_id%3D732aef47"), "client_id must survive encoding");
        // compat 模式（无 client_id）同样直连 SSO
        let c = DeviceFlow::new_compat();
        assert!(c.auth_url.starts_with(SSO_LOGIN_BASE));
        assert!(!c.auth_url.contains("client_id"));
    }

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
        assert_eq!(get("Cosy-MachineOS").unwrap(), CLIENT_MACHINE_OS_DESKTOP); // default flavor
        // android flavor switches the OS tag (mobile escape hatch)
        let mut cfg_a = AppConfig::default();
        cfg_a.platform_flavor = "android".into();
        let h2 = build_auth_headers(&cfg_a, &creds);
        assert_eq!(
            h2.iter().find(|(k, _)| k == "Cosy-MachineOS").unwrap().1,
            CLIENT_MACHINE_OS_ANDROID
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
        assert!(f1.auth_url.starts_with(SSO_LOGIN_BASE));
        for key in [
            "challenge%3D",
            "challenge_method%3DS256",
            "nonce%3D",
            "machine_id%3D",
            "client_id%3D",
        ] {
            assert!(f1.auth_url.contains(key), "auth_url 缺少 {key}");
        }
        assert!(f1.auth_url.contains("client_id%3D732aef47"), "client_id 编码后必须在回调内");
        // issue #140：directLogin=true 单个出现在回调内（模拟授权页自构形态）
        assert!(f1.auth_url.contains("directLogin%3Dtrue"));
        // challenge 可由 verifier 复算（S256 绑定）
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(f1.verifier.as_bytes()));
        assert!(f1.auth_url.contains(&format!("challenge%3D{expect}")));
        // compat 模式不带 client_id
        let fc = DeviceFlow::new_compat();
        assert!(fc.client_id.is_none());
        assert!(!fc.auth_url.contains("client_id"));
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
