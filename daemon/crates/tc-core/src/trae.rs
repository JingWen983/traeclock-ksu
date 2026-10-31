//! Trae multi-account check-in — port of `tasks/trae_checkin.rs` (plus its
//! python ancestor semantics): status pre-check → claim → error classify →
//! cooldown → credits three-layer fallback.
//!
//! Mobile adaptation (the ONE intentional behavior change): x-lscbd-platform
//! and friends switch to the android flavor by default; every header remains
//! overridable via `AppConfig.header_overrides`, and `platform_flavor =
//! "windows"` restores the desktop set verbatim.
//! Red line: JWT never enters logs/events.

use serde_json::{json, Value};
use chrono::TimeZone;

use crate::config::AppConfig;
use crate::envelope::{as_int_tolerant, find_payload_field, parse_claim_reward};
use crate::error::{
    classify_error, humanize_checkin_error, is_auth_rejected_error, is_device_signed_error,
    secs_until_next_checkin_reset, CooldownState,
};
use crate::events::{Event, Sink};
use crate::fingerprint::DeviceEntry;
use crate::http;
use crate::jwt;
use crate::store::{Account, CreditRecord, Store};
use crate::util::random_hex;

pub const SIGNIN_URL: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits/claim";
pub const STATUS_URL: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits/status";
/// JWT remaining validity below this (hours) raises a warning.
pub const EXPIRY_WARN_HOURS: f64 = 24.0;
/// Network-error retry gap (matches source RETRY_GAP).
pub const RETRY_GAP_SECS: u64 = 1;

/// Device triple for uid: device_map entry wins (drift guard), else the
/// module identity device_id (config.module_device_id) — Trae JWTs are issued
/// against the OAuth login URL's device_id = module identity; a per-uid
/// derived id has never been seen by the server and /pay/ endpoints reject
/// it with 401. Legacy accounts logged in before the device_map writeback
/// (or whose device_map entry predates the identity binding) land here.
pub fn device_for(cfg: &AppConfig, uid: &str) -> DeviceEntry {
    if let Some(e) = cfg.device_map.get(uid) {
        if !e.device_id.is_empty() {
            return e.clone();
        }
    }
    let mut e = crate::fingerprint::derive_device(uid);
    if !cfg.module_device_id.is_empty() {
        e.device_id = cfg.module_device_id.clone();
    }
    e
}

/// Check-in / status shared headers (per-account device id + session).
/// Port of `build_headers`; android flavor swaps the platform-identifying trio.
pub fn build_headers(cfg: &AppConfig, jwt: &str, dev: &DeviceEntry) -> Vec<(String, String)> {
    let auth = if jwt.starts_with("Cloud-IDE-JWT ") {
        jwt.to_string()
    } else {
        format!("Cloud-IDE-JWT {}", jwt.trim())
    };
    let (lscbd_platform, device_type, sec_fetch_dest) = if cfg.platform_flavor == "windows" {
        ("windows", "windows", "empty")
    } else {
        ("android", "android", "empty")
    };
    let mut h = vec![
        ("accept".to_string(), "*/*".to_string()),
        ("accept-language".to_string(), "zh-CN".to_string()),
        ("authorization".to_string(), auth),
        ("content-type".to_string(), "application/json".to_string()),
        ("user-agent".to_string(), "VSCode 1.107.1 (TRAE SOLO CN)".to_string()),
        ("x-market-client-id".to_string(), "VSCode 1.107.1".to_string()),
        ("x-market-user-id".to_string(), dev.market_user_id.clone().unwrap_or_default()),
        ("x-user-region".to_string(), "CN".to_string()),
        ("x-device-id".to_string(), dev.device_id.clone()),
        ("x-lgw-req-sdk-type".to_string(), "3".to_string()),
        ("package-type".to_string(), "stable_cn".to_string()),
        ("x-request-id".to_string(), random_hex(32)),
        ("x-lscbd-aid".to_string(), "787976".to_string()),
        ("x-lscbd-platform".to_string(), lscbd_platform.to_string()),
        ("app-version".to_string(), "0.1.45".to_string()),
        ("x-tt-trace-id".to_string(), format!("00-{}-01", random_hex(16))),
        ("vscode-sessionid".to_string(), dev.session_id.clone().unwrap_or_default()),
        ("x-device-type".to_string(), device_type.to_string()),
        ("sec-fetch-dest".to_string(), sec_fetch_dest.to_string()),
        ("sec-fetch-mode".to_string(), "no-cors".to_string()),
        ("sec-fetch-site".to_string(), "none".to_string()),
    ];
    cfg.apply_overrides("trae", &mut h);
    h
}

struct StatusOutcome {
    ok: bool,
    checked_in: Option<bool>,
    credits: Option<i64>,
    message: String,
}

fn status_check(agent: &ureq::Agent, cfg: &AppConfig, jwt: &str, dev: &DeviceEntry) -> StatusOutcome {
    let none = |message: &str| StatusOutcome {
        ok: false,
        checked_in: None,
        credits: None,
        message: message.to_string(),
    };
    let (status, body, raw) = http::post_json(agent, STATUS_URL, &build_headers(cfg, jwt, dev), &json!({}));
    if status == 0 {
        let msg = if raw.is_empty() { "网络异常".to_string() } else { raw };
        return none(&msg);
    }
    let Some(data) = body.filter(|b| b.is_object()) else {
        let head: String = raw.chars().take(200).collect();
        return none(&format!("非 JSON 响应: {head}"));
    };
    let code = data.get("code").and_then(Value::as_i64);
    let checked_in = match find_payload_field(&data, "checked_in") {
        Some(v) if v.is_boolean() => v.as_bool(),
        Some(v) => v.as_f64().map(|f| f != 0.0),
        None => None,
    };
    let credits = find_payload_field(&data, "credits").and_then(as_int_tolerant);
    let msg = data.get("message").and_then(Value::as_str).unwrap_or("").to_string();
    if code != Some(0) {
        return StatusOutcome {
            ok: false,
            checked_in,
            credits,
            message: if msg.is_empty() { format!("HTTP {status}") } else { humanize_checkin_error(&msg) },
        };
    }
    StatusOutcome { ok: true, checked_in, credits, message: msg }
}

/// Claim request + parse → (ok, msg, code, http_status, data).
/// code == None marks a transport-level failure (retryable).
#[allow(clippy::type_complexity)]
fn signin_request(
    agent: &ureq::Agent,
    cfg: &AppConfig,
    jwt: &str,
    dev: &DeviceEntry,
) -> (bool, String, Option<i64>, i32, Option<Value>) {
    let (status, body, raw) = http::post_json(agent, SIGNIN_URL, &build_headers(cfg, jwt, dev), &json!({}));
    if status == 0 {
        let msg = if raw.is_empty() { "网络异常".to_string() } else { raw };
        return (false, msg, None, 0, None);
    }
    match body.filter(|b| b.is_object()) {
        Some(data) => {
            let code = data.get("code").and_then(Value::as_i64);
            let msg = data
                .get("message")
                .and_then(Value::as_str)
                .map(humanize_checkin_error)
                .unwrap_or_else(|| format!("HTTP {status}"));
            (code == Some(0), msg, code, status, Some(data))
        }
        None => {
            let head: String = raw.chars().take(200).collect();
            (false, format!("HTTP {status}: 非 JSON 响应: {head}"), None, status, None)
        }
    }
}

/// Claim with network retries: only transport failures (code == None) retry.
#[allow(clippy::type_complexity)]
fn signin_with_retry(
    agent: &ureq::Agent,
    cfg: &AppConfig,
    jwt: &str,
    dev: &DeviceEntry,
    retry: u32,
) -> (bool, String, Option<i64>, i32, Option<Value>) {
    let mut last = (false, "无重试".to_string(), None, 0, None);
    for attempt in 0..=retry {
        last = signin_request(agent, cfg, jwt, dev);
        if last.0 || last.2.is_some() {
            return last;
        }
        if attempt < retry {
            std::thread::sleep(std::time::Duration::from_secs(RETRY_GAP_SECS));
        }
    }
    last
}

/// Credits attribution three-layer fallback (pure function, port of
/// `resolve_claim_credits`): claim reward field → balance diff recheck → legacy.
fn resolve_claim_credits(
    claim_data: Option<&Value>,
    credits_before: Option<i64>,
    mut recheck: Option<&mut dyn FnMut() -> (bool, Option<i64>)>,
) -> (Option<i64>, i64, &'static str) {
    if let Some(reward) = parse_claim_reward(claim_data) {
        return (credits_before, reward, "claim_reward");
    }
    if let (Some(before), Some(re)) = (credits_before, recheck.as_deref_mut()) {
        let (ok_r, credits_after) = re();
        if ok_r && credits_after.is_some_and(|c| c - before >= 0) {
            let after = credits_after.unwrap();
            return (Some(after), after - before, "balance_diff");
        }
    }
    match credits_before {
        Some(c) => (Some(c), c, "legacy"),
        None => (None, 0, "legacy"),
    }
}

/// Per-account execution outcome (used by retry-round merging at the caller).
#[derive(Debug, Clone, PartialEq)]
pub enum AccountStatus {
    Success,
    Already,
    Fail,
}

/// One account's check-in, resolved device fingerprints fresh from cfg.
/// Returns (status, user_id, display message, credits, delta, error_type, cooldown_seconds).
#[allow(clippy::too_many_arguments)]
fn checkin_one(
    agent: &ureq::Agent,
    cfg: &AppConfig,
    cooldowns: &mut CooldownState,
    acc: &Account,
) -> (AccountStatus, String, String, Option<i64>, i64, Option<&'static str>, i64) {
    let name = if acc.name.is_empty() { acc.user_id.clone() } else { acc.name.clone() };
    let Some(jwt_raw) = acc.jwt.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    else {
        return (
            AccountStatus::Fail,
            acc.user_id.clone(),
            name,
            None,
            0,
            None,
            0,
        );
    };
    let info = jwt::parse(&jwt_raw);
    let uid = info.user_id.clone().unwrap_or_default();
    let dev = device_for(cfg, &uid);

    // status pre-check: skip already-checked-in accounts
    let pre = status_check(agent, cfg, &jwt_raw, &dev);
    if pre.ok && pre.checked_in == Some(true) {
        return (AccountStatus::Already, uid, pre.message, pre.credits, 0, None, 0);
    }

    // claim
    let (ok, msg, code, http_status, claim_data) =
        signin_with_retry(agent, cfg, &jwt_raw, &dev, cfg.retry);
    if ok {
        cooldowns.clear(&uid);
        let mut recheck = || {
            let r = status_check(agent, cfg, &jwt_raw, &dev);
            (r.ok, r.credits)
        };
        let (credits, delta, _src) =
            resolve_claim_credits(claim_data.as_ref(), pre.credits, Some(&mut recheck));
        return (AccountStatus::Success, uid, msg, credits, delta, None, 0);
    }
    let (mut error_type, mut cooldown_secs) = classify_error(http_status, code);
    // 「服务端拒绝认证」（issue #128）：HTTP 200 + 业务码拒绝的凭证级失效，重试与
    // 短冷却均无法自愈——单列永久类型（对齐 SessionDead 口径，需重走 OAuth）
    if is_auth_rejected_error(&msg) {
        error_type = "AuthRejected";
        cooldown_secs = -1;
    }
    // 「设备当日已签」（issue #110）：当日去重不因重试解除，冷却拨到明日 00:05，
    // 避免固定时长跨日误挡次日签到
    if is_device_signed_error(&msg) {
        error_type = "DeviceSigned";
        cooldown_secs = secs_until_next_checkin_reset();
    }
    if error_type != "Unknown" {
        cooldowns.update(&uid, error_type, cooldown_secs, &msg);
    }
    (
        AccountStatus::Fail,
        uid,
        msg,
        None,
        0,
        if error_type == "Unknown" { None } else { Some(error_type) },
        cooldown_secs,
    )
}

/// One Trae round: serial per account, gap between accounts, failures isolated.
/// Persists cooldowns / state / summary; emits the NDJSON event pipeline.
pub fn run_round(store: &Store, cfg: &AppConfig, sink: &mut dyn Sink) -> Value {
    let accounts_all = store.accounts();
    let mut accounts: Vec<Account> = accounts_all
        .accounts
        .iter()
        .filter(|a| a.app == crate::store::APP_TRAE && a.enabled)
        .cloned()
        .collect();
    // skip accounts that are currently cooling down or permanently dead
    let now_ts = chrono::Utc::now().timestamp();
    let mut cooldowns = CooldownState { map: store.cooldowns() };
    accounts.retain(|a| {
        let uid = jwt::parse(a.jwt.as_deref().unwrap_or("")).user_id.unwrap_or_else(|| a.user_id.clone());
        !cooldowns.is_cooling(&uid, now_ts)
    });

    let agent = http::agent(30);
    let mut ok = 0usize;
    let mut already = 0usize;
    let mut failed = 0usize;
    let mut warnings: Vec<String> = Vec::new();
    let mut results: Vec<Value> = Vec::with_capacity(accounts.len());
    let mut state = store.state();

    sink.emit(&Event::start(accounts.len()));

    for (i, acc) in accounts.iter().enumerate() {
        if i > 0 && cfg.account_gap_secs > 0 {
            std::thread::sleep(std::time::Duration::from_secs(cfg.account_gap_secs));
        }
        let idx = i + 1;
        let name = if acc.name.is_empty() { format!("账号{idx}") } else { acc.name.clone() };

        // JWT expiry warning (warnings only; never blocks)
        if let Some(j) = acc.jwt.as_deref() {
            let info = jwt::parse(j);
            if let (Some(h), Some(exp)) = (info.exp_hours, info.exp_timestamp) {
                let exp_txt = chrono::Local
                    .timestamp_opt(exp, 0)
                    .single()
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| exp.to_string());
                if h < 0.0 {
                    warnings.push(format!("{name}: JWT 已过期({exp_txt})，请重新登录"));
                } else if h < EXPIRY_WARN_HOURS {
                    warnings.push(format!("{name}: JWT 将于 {h:.1}h 后过期({exp_txt})"));
                }
            }
        }

        let (status, uid, msg, credits, delta, error_type, cd_secs) =
            checkin_one(&agent, cfg, &mut cooldowns, acc);
        match status {
            AccountStatus::Already => {
                already += 1;
                if let Some(c) = credits {
                    state.push_credit(CreditRecord {
                        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
                        user_id: uid.clone(),
                        credits: c,
                        delta: 0,
                    });
                }
                sink.emit(&Event::account_already(idx, &uid, &name, credits));
                results.push(json!({
                    "name": name, "user_id": uid, "ok": true, "code": 0,
                    "action": "skip_already", "credits": credits, "message": msg,
                }));
            }
            AccountStatus::Success => {
                ok += 1;
                if let Some(c) = credits {
                    state.push_credit(CreditRecord {
                        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
                        user_id: uid.clone(),
                        credits: c,
                        delta,
                    });
                }
                sink.emit(&Event::account_success(idx, &uid, &name, Some(0), &msg, credits, Some(delta)));
                results.push(json!({
                    "name": name, "user_id": uid, "ok": true, "code": 0,
                    "action": "claim_ok", "credits": credits, "credits_delta": delta, "message": msg,
                }));
            }
            AccountStatus::Fail => {
                failed += 1;
                let until = if cd_secs > 0 {
                    Some(chrono::Utc::now().timestamp() + cd_secs)
                } else if cd_secs < 0 {
                    Some(9_999_999_999)
                } else {
                    None
                };
                sink.emit(&Event::account_fail(idx, &uid, &name, &msg, None, error_type, until));
                results.push(json!({
                    "name": name, "user_id": uid, "ok": false, "message": msg,
                    "error_type": error_type, "cooldown_until": until,
                }));
            }
        }
    }

    // persist: cooldowns / state / summary log
    let _ = store.save_cooldowns(&cooldowns.map);
    let _ = store.save_state(&state);
    let now_txt = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut line = format!("[{now_txt}] [trae] 成功{ok}/已签到{already}/失败{failed}/总计{}", accounts.len());
    if !warnings.is_empty() {
        line.push_str(&format!(" | 告警: {}", warnings.join("; ")));
    }
    store.append_checkin_log(&line);
    store.append_event(&Event::done(ok, already, failed).to_line());

    let done = Event::done(ok, already, failed).to_json();
    sink.emit(&Event {
        kind: "done".into(),
        fields: json!({
            "ok": ok, "already": already, "failed": failed,
            "results": results, "warnings": warnings,
        }),
    });
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_credits_fallback_layers() {
        // layer 1: claim reward field
        let (c, d, s) = resolve_claim_credits(Some(&json!({"reward": 5})), Some(100), None);
        assert_eq!((c, d, s), (Some(100), 5, "claim_reward"));
        // layer 2: balance diff via recheck
        let mut re = || (true, Some(105i64));
        let (c, d, s) = resolve_claim_credits(Some(&json!({})), Some(100), Some(&mut re));
        assert_eq!((c, d, s), (Some(105), 5, "balance_diff"));
        // layer 2 negative diff → layer 3
        let mut re = || (true, Some(90i64));
        let (c, d, s) = resolve_claim_credits(Some(&json!({})), Some(100), Some(&mut re));
        assert_eq!((c, d, s), (Some(100), 100, "legacy"));
        // layer 3 no prior balance
        let (c, d, s) = resolve_claim_credits(Some(&json!({})), None, None);
        assert_eq!((c, d, s), (None, 0, "legacy"));
    }

    #[test]
    fn headers_carry_desktop_flavor() {
        let mut cfg = AppConfig::default();
        cfg.platform_flavor = "windows".into();
        let dev = crate::fingerprint::derive_device("u1");
        let h = build_headers(&cfg, "tok", &dev);
        let get = |k: &str| h.iter().find(|(hk, _)| hk == k).map(|(_, v)| v.clone());
        assert_eq!(get("authorization").unwrap(), "Cloud-IDE-JWT tok");
        assert_eq!(get("x-lscbd-platform").unwrap(), "windows");
        assert_eq!(get("x-device-id").unwrap(), dev.device_id);
        assert_eq!(get("x-lscbd-aid").unwrap(), "787976");
        // request id / trace id are unique per call
        let h2 = build_headers(&cfg, "tok", &dev);
        assert_ne!(get("x-request-id").unwrap(), h2.iter().find(|(k, _)| k == "x-request-id").unwrap().1);
        // prefixed jwt is not double-prefixed
        let h3 = build_headers(&cfg, "Cloud-IDE-JWT tok", &dev);
        assert_eq!(h3.iter().find(|(k, _)| k == "authorization").unwrap().1, "Cloud-IDE-JWT tok");
    }

    #[test]
    fn headers_carry_android_flavor_and_overrides() {
        let mut cfg = AppConfig::default();
        cfg.platform_flavor = "android".into();
        cfg.header_overrides
            .insert("trae:x-lscbd-platform".into(), "windows".into());
        let dev = crate::fingerprint::derive_device("u1");
        let h = build_headers(&cfg, "tok", &dev);
        let get = |k: &str| h.iter().find(|(hk, _)| hk == k).map(|(_, v)| v.clone());
        // override wins over android default
        assert_eq!(get("x-lscbd-platform").unwrap(), "windows");
        assert_eq!(get("x-device-type").unwrap(), "android");
    }

    #[test]
    fn trace_id_shape() {
        use crate::util::to_hex_lower;
        // "00-<32 hex chars>-01": random_hex(16) → 32 chars
        let t = format!("00-{}-01", to_hex_lower(&[0u8; 16]));
        let parts: Vec<&str> = t.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "00");
        assert_eq!(parts[2], "01");
    }
}
