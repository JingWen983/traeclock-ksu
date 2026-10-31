//! WorkBuddy / CodeBuddy check-in + credential lifecycle — port of
//! `tasks/wb_common.rs` (headers, fingerprints, region routing, refresh) and
//! `tasks/wb_checkin.rs` (status → do → 401-refresh-once-retry → reward
//! tolerant parsing → balance-diff fallback).
//!
//! Simplifications vs desktop (documented, behavior-preserving where it
//! matters): no desktop auth-file double source (the module store is the only
//! source), no cross-process named mutex (single daemon), WB-credits-domain
//! balance layer omitted (billing meter diff retained).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::AppConfig;
use crate::envelope::dig;
use crate::events::{Event, Sink};
use crate::http;
use crate::store::{Account, CreditRecord, Store, APP_WB};
use crate::util::sha256_hex;

// ── constants (port of wb_common) ────────────────────────────────────────────

/// Desktop UA must carry a version — bare "WorkBuddy" is detected as "-" in
/// the personal-center request log (issue #48, a detectable fingerprint).
pub const WB_DESKTOP_UA: &str = "WorkBuddy/5.5.6";

pub const BILLING_BASE_CN: &str = "https://www.codebuddy.cn";
pub const BILLING_BASE_GLOBAL: &str = "https://www.workbuddy.ai";
pub const REFRESH_URL: &str = "https://www.codebuddy.cn/v2/plugin/auth/token/refresh";

pub fn is_global_region(domain: &str) -> bool {
    domain.contains(".workbuddy.ai")
}

pub fn region_billing_base(domain: &str) -> &'static str {
    if is_global_region(domain) {
        BILLING_BASE_GLOBAL
    } else {
        BILLING_BASE_CN
    }
}

/// Domain dual-probe: main base first, fallback second.
pub fn billing_bases(domain: &str) -> [&'static str; 2] {
    let main = region_billing_base(domain);
    let alt = if main == BILLING_BASE_CN { BILLING_BASE_GLOBAL } else { BILLING_BASE_CN };
    [main, alt]
}

/// Account-stable device fingerprint (port of `derive_device_fingerprint`):
/// sha256("wb-fingerprint:{kind}:{uid}") first 16 bytes → 32 hex chars.
pub fn derive_device_fingerprint(uid: &str, kind: &str) -> Option<String> {
    if uid.trim().is_empty() {
        return None;
    }
    Some(sha256_hex(&format!("wb-fingerprint:{kind}:{uid}"))[..32].to_string())
}

// ── creds ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Creds {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub refresh_expires_at_ms: Option<i64>,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub edition: String,
}

impl Creds {
    pub fn from_account(a: &Account) -> Creds {
        Creds {
            access_token: a.access_token.clone().unwrap_or_default(),
            refresh_token: a.refresh_token.clone().unwrap_or_default(),
            expires_at_ms: a.access_token_expires_at.map(|s| s * 1000),
            refresh_expires_at_ms: a.refresh_token_expires_at.map(|s| s * 1000),
            uid: a.user_id.clone(),
            domain: a.domain.clone().unwrap_or_default(),
            nickname: a.name.clone(),
            edition: String::new(),
        }
    }
}

/// Auth headers (port of `build_auth_headers`): Bearer + X-User-Id + stable
/// machine/session fingerprints + X-Domain; web_platform adds X-Client-Platform.
pub fn build_auth_headers(
    cfg: &AppConfig,
    creds: &Creds,
    web_platform: bool,
) -> Vec<(String, String)> {
    let mut h = vec![
        ("Authorization".to_string(), format!("Bearer {}", creds.access_token)),
        ("User-Agent".to_string(), WB_DESKTOP_UA.to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    if creds.uid.is_empty() {
        h.push(("X-No-User-Id".to_string(), "1".to_string()));
    } else {
        h.push(("X-User-Id".to_string(), creds.uid.clone()));
        if let Some(mid) = derive_device_fingerprint(&creds.uid, "machine") {
            h.push(("X-Machine-ID".to_string(), mid));
        }
        if let Some(sid) = derive_device_fingerprint(&creds.uid, "session") {
            h.push(("X-Session-ID".to_string(), sid));
        }
    }
    if !creds.domain.is_empty() {
        h.push(("X-Domain".to_string(), creds.domain.clone()));
    }
    if web_platform {
        h.push(("X-Client-Platform".to_string(), "web".to_string()));
    }
    cfg.apply_overrides("wb", &mut h);
    h
}

// ── refresh (F-09) ───────────────────────────────────────────────────────────

/// Refresh failure taxonomy (port of `RefreshFail`): only `Auth` may mark
/// needs_relogin — network failures must not kill credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFail {
    NoRefreshToken,
    Network,
    Auth,
    BadResponse,
}

/// POST plugin refresh (X-Refresh-Token only allowed on this endpoint).
/// New expires fields backfilled from expiresIn/refreshExpiresIn seconds.
/// Third item = upstream `error`/`error_description` detail (issue #118): lets the
/// caller tell "session expired" apart from "account banned" instead of guessing.
pub fn refresh_token_once_ex(
    agent: &ureq::Agent,
    creds: &Creds,
) -> (Option<Creds>, RefreshFail, Option<String>) {
    if creds.refresh_token.is_empty() {
        return (None, RefreshFail::NoRefreshToken, None);
    }
    let mut h = build_auth_headers(&crate::config::AppConfig::default(), creds, false);
    h.push(("X-Refresh-Token".to_string(), creds.refresh_token.clone()));
    h.push(("X-Auth-Refresh-Source".to_string(), "workbuddy".to_string()));
    let (status, body, _raw) = http::post_json(agent, REFRESH_URL, &h, &json!({}));
    if status == 0 {
        return (None, RefreshFail::Network, None);
    }
    if (400..500).contains(&status) {
        return (None, RefreshFail::Auth, refresh_detail(body.as_ref()));
    }
    if status != 200 {
        return (None, RefreshFail::Network, None); // 5xx etc.: do not mark relogin
    }
    let Some(body) = body else {
        return (None, RefreshFail::BadResponse, None);
    };
    let acc = dig(&body, &["accessToken"]).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if acc.is_empty() {
        return (None, RefreshFail::BadResponse, refresh_detail(Some(&body)));
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut out = creds.clone();
    out.access_token = acc;
    let ref_tok = dig(&body, &["refreshToken"]).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !ref_tok.is_empty() {
        out.refresh_token = ref_tok;
    }
    out.expires_at_ms =
        dig(&body, &["expiresIn"]).and_then(|v| v.as_i64()).map(|e| now_ms + e * 1000);
    out.refresh_expires_at_ms =
        dig(&body, &["refreshExpiresIn"]).and_then(|v| v.as_i64()).map(|e| now_ms + e * 1000);
    (Some(out), RefreshFail::NoRefreshToken, None)
}

/// 上游失败详情透传（issue #118）：取 `error` / `error_description` / `message`，
/// 用于把「会话失效」与「账号封禁」区分开（仅诊断用途，不改变分类）。
fn refresh_detail(body: Option<&Value>) -> Option<String> {
    let b = body?;
    for key in ["error_description", "error", "message"] {
        if let Some(s) = dig(b, &[key]).and_then(Value::as_str) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Lazy renewal gate (port of `lazy_renew_skippable`): access_token remaining
/// > lazy_hours → skip refresh (fewer upstream token fetches).
pub fn lazy_renew_skippable(expires_at_ms: Option<i64>, lazy_hours: i64, now_ms: i64) -> bool {
    match expires_at_ms {
        Some(exp) => {
            let remain_h = (exp - now_ms) as f64 / 3_600_000.0;
            remain_h > lazy_hours as f64
        }
        None => false, // unknown expiry → try refresh path
    }
}

// ── endpoints (port of wb_checkin urls / status / do) ────────────────────────

struct Urls {
    base: &'static str,
    checkin_status: String,
    checkin_status_old: String,
    checkin_do: String,
}

fn urls_for(base: &'static str) -> Urls {
    Urls {
        base,
        checkin_status: format!("{base}/v2/billing/meter/checkin-activity-status"),
        checkin_status_old: format!("{base}/v2/billing/meter/checkin-status"),
        checkin_do: format!("{base}/v2/billing/meter/daily-checkin"),
    }
}

impl Urls {
    fn alt(&self) -> Urls {
        let [main, alt_b] = billing_bases("");
        let alt = if main == self.base { alt_b } else { main };
        urls_for(alt)
    }
}

fn s_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").to_string()
}

/// python bool() semantics (0/""/null/false → false, else true).
fn py_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map_or(true, |f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

fn num_or_none(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    match v {
        Value::Bool(_) => None,
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Recursive reward deep-dig: numeric fields whose key hints a reward
/// (deliberately excludes balance/total/remaining — never mistake the
/// account balance for this round's reward).
fn deep_reward_dig(v: &Value, depth: usize) -> Option<f64> {
    if depth > 4 {
        return None;
    }
    const HINTS: [&str; 10] = [
        "reward", "integral", "score", "bonus", "earn", "gain", "obtain", "add", "claim", "credit",
    ];
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                let kl = k.to_ascii_lowercase();
                if HINTS.iter().any(|h| kl.contains(h)) {
                    if let Some(n) = num_or_none(Some(val)) {
                        return Some(n);
                    }
                }
                if let Some(n) = deep_reward_dig(val, depth + 1) {
                    return Some(n);
                }
            }
            None
        }
        Value::Array(a) => a.iter().find_map(|x| deep_reward_dig(x, depth + 1)),
        _ => None,
    }
}

/// Today checked-in? New path falls back to old path. → (checked, status_ok)
fn checkin_status(agent: &ureq::Agent, headers: &[(String, String)], urls: &Urls) -> (Option<bool>, bool) {
    for url in [&urls.checkin_status, &urls.checkin_status_old] {
        let (status, body, _raw) = http::post_json(agent, url, headers, &json!({}));
        if status == 401 {
            return (None, false);
        }
        if status == 200 {
            if let Some(b) = body.filter(|b| b.is_object()) {
                let mut v = dig(&b, &["today_checked_in", "todayCheckedIn", "checked_in", "checkedIn"]);
                if v.is_none() {
                    v = dig(&b, &["checked", "is_checked"]);
                }
                if let Some(v) = v {
                    return (Some(py_truthy(Some(v))), true);
                }
                return (None, true); // 200 but field missing: unknown, not fatal
            }
        }
        // other statuses try the old path
    }
    (None, false)
}

/// Execute check-in → (kind, message, reward, dbg_keys).
/// kind ∈ success / already / fail / auth (caller refreshes + retries once).
fn checkin_do(
    agent: &ureq::Agent,
    headers: &[(String, String)],
    urls: &Urls,
) -> (String, String, Option<f64>, Option<String>) {
    let (mut status, mut body, mut raw) = http::post_json(agent, &urls.checkin_do, headers, &json!({}));
    if status == 0 {
        let alt = urls.alt().checkin_do;
        if alt != urls.checkin_do {
            let r = http::post_json(agent, &alt, headers, &json!({}));
            status = r.0;
            body = r.1;
            raw = r.2;
        }
    }
    if status == 401 {
        return ("auth".into(), "登录态失效（401）".into(), None, None);
    }
    let code: Option<i64> = body
        .as_ref()
        .and_then(|b| b.get("code"))
        .and_then(Value::as_i64)
        .or_else(|| body.as_ref().and_then(|b| dig(b, &["code"])).and_then(Value::as_i64));
    let message = body.as_ref().and_then(|b| dig(b, &["message", "msg"])).map(|v| s_of(Some(v)));
    if status == 0 {
        let head: String = raw.chars().take(120).collect();
        return ("fail".into(), format!("网络不可达: {head}"), None, None);
    }
    if (200..=201).contains(&status) || code.is_some_and(|c| c == 0 || c == 200) {
        let reward = body.as_ref().and_then(|b| {
            num_or_none(dig(
                b,
                &[
                    "reward", "credits", "credit", "points", "amount", "integral", "score", "bonus",
                    "reward_amount", "add_integral", "addCredits", "earned",
                ],
            ))
            .or_else(|| deep_reward_dig(b, 0))
        });
        let dbg_keys = if reward.is_none() {
            body.as_ref().map(|b| {
                let mut paths = Vec::new();
                collect_key_paths(b, "", 0, &mut paths);
                paths.join(" | ")
            })
        } else {
            None
        };
        return ("success".into(), "签到成功".into(), reward, dbg_keys);
    }
    // already-checked-in tolerance (F-15): code:10001 / message contains 已签到 / repeat
    let msg_low = message.as_deref().unwrap_or("").to_ascii_lowercase();
    if code == Some(10001)
        || ["已签到", "repeat", "already"].iter().any(|k| msg_low.contains(k))
    {
        return ("already".into(), "今日已签到".into(), None, None);
    }
    let head: String = raw.chars().take(120).collect();
    let shown = message
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| if head.is_empty() { format!("HTTP {status}") } else { head });
    (
        "fail".into(),
        format!("{shown}（code={}）", code.map(|c| c.to_string()).unwrap_or_else(|| "None".into())),
        None,
        None,
    )
}

/// Collect JSON key paths (depth/count capped; keys only, never values —
/// diagnostics must not leak sensitive payloads).
fn collect_key_paths(v: &Value, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > 6 || out.len() >= 40 {
        return;
    }
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                out.push(p.clone());
                collect_key_paths(val, &p, depth + 1, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate().take(3) {
                collect_key_paths(x, &format!("{prefix}[{i}]"), depth + 1, out);
            }
        }
        _ => {}
    }
}

/// Billing-meter balance query (for reward-by-diff fallback). None on failure.
fn fetch_balance(agent: &ureq::Agent, headers: &[(String, String)], base: &str) -> Option<f64> {
    let path = "/billing/meter/get-user-resource-summary";
    let (mut status, mut body, _raw) =
        http::post_json(agent, &format!("{base}{path}"), headers, &json!({}));
    if status == 0 {
        let alt = if base == BILLING_BASE_CN { BILLING_BASE_GLOBAL } else { BILLING_BASE_CN };
        if alt != base {
            let r = http::post_json(agent, &format!("{alt}{path}"), headers, &json!({}));
            status = r.0;
            body = r.1;
        }
    }
    if status != 200 {
        return None;
    }
    let b = body?;
    num_or_none(dig(&b, &["RemainingCapacity", "remaining", "TotalRemaining", "Balance", "balance"]))
}

// ── round ────────────────────────────────────────────────────────────────────

/// One WorkBuddy round (port of `run_checkin_round` core): serial accounts,
/// 401 → refresh-once → single retry (never a second refresh, F-09),
/// needs_relogin marked only on confirmed credential death.
pub fn run_round(store: &Store, cfg: &AppConfig, sink: &mut dyn Sink) -> Value {
    let accounts_file = store.accounts();
    let accounts: Vec<Account> = accounts_file
        .accounts
        .iter()
        .filter(|a| a.app == APP_WB && a.enabled)
        .cloned()
        .collect();
    let agent = http::agent(30);
    let lazy_hours = 24i64;
    let mut ok = 0usize;
    let mut already = 0usize;
    let mut failed = 0usize;
    let mut state = store.state();
    let mut accounts_file_mut = accounts_file.clone();

    sink.emit(&Event::start(accounts.len()));

    for (i, acc) in accounts.iter().enumerate() {
        if i > 0 && cfg.account_gap_secs > 0 {
            std::thread::sleep(std::time::Duration::from_secs(cfg.account_gap_secs));
        }
        let idx = i + 1;
        let aid = acc.user_id.clone();
        let name = if acc.name.is_empty() { aid.chars().take(8).collect() } else { acc.name.clone() };

        if acc.needs_relogin {
            failed += 1;
            sink.emit(&Event::account_fail(idx, &aid, &name, "需重新登录，已跳过", None, None, None));
            continue;
        }
        let mut creds = Creds::from_account(acc);
        if creds.access_token.is_empty() {
            failed += 1;
            sink.emit(&Event::account_fail(idx, &aid, &name, "无可用凭证（no_credential）", None, None, None));
            continue;
        }
        // lazy refresh gate
        let now_ms = chrono::Utc::now().timestamp_millis();
        if !lazy_renew_skippable(creds.expires_at_ms, lazy_hours, now_ms)
            && !creds.refresh_token.is_empty()
        {
            if let Some(new) = refresh_token_once_ex(&agent, &creds).0 {
                creds = new;
                persist_creds(store, &mut accounts_file_mut, &aid, &creds);
            }
        }

        let base = region_billing_base(&creds.domain);
        let urls = urls_for(base);
        let mut headers = build_auth_headers(cfg, &creds, false);
        let (checked, ok_status) = checkin_status(&agent, &headers, &urls);
        if ok_status && checked == Some(true) {
            already += 1;
            sink.emit(&Event::account_already(idx, &aid, &name, None));
            continue;
        }

        let pre_balance = fetch_balance(&agent, &headers, base);
        let (mut kind, mut message, mut reward, mut dbg_keys) =
            checkin_do(&agent, &headers, &urls);
        if kind == "auth" {
            // 401: refresh once, retry the failure branch only (no second refresh)
            match refresh_token_once_ex(&agent, &creds) {
                (Some(new), _, _) => {
                    persist_creds(store, &mut accounts_file_mut, &aid, &new);
                    creds = new;
                    headers = build_auth_headers(cfg, &creds, false);
                    let r = checkin_do(&agent, &headers, &urls);
                    kind = r.0;
                    message = r.1;
                    reward = r.2;
                    dbg_keys = r.3;
                    if kind == "auth" {
                        mark_needs_relogin(store, &mut accounts_file_mut, &aid, "签到重试后仍 401");
                        kind = "fail".into();
                        message = "登录态失效（刷新后仍 401），需重新登录".into();
                        reward = None;
                    }
                }
                (None, RefreshFail::Auth, detail) => {
                    // issue #118：透传上游 error/error_description，区分会话失效与账号封禁
                    let mut reason = "签到 refresh 失败".to_string();
                    if let Some(d) = detail.as_deref() {
                        reason.push_str(&format!("；上游: {d}"));
                    }
                    mark_needs_relogin(store, &mut accounts_file_mut, &aid, &reason);
                    kind = "fail".into();
                    message = match detail.as_deref() {
                        Some(d) => format!("登录态失效且刷新失败，需重新登录；上游: {d}"),
                        None => "登录态失效且刷新失败，需重新登录".into(),
                    };
                    reward = None;
                }
                (None, _, _) => {
                    kind = "fail".into();
                    message = "登录态刷新失败（网络），稍后重试".into();
                    reward = None;
                }
            }
        }
        // reward-by-diff fallback: only a positive diff is trusted
        if kind == "success" && reward.is_none() {
            if let Some(pre) = pre_balance {
                if let Some(post) = fetch_balance(&agent, &headers, base) {
                    if post > pre {
                        reward = Some(((post - pre) * 100.0).round() / 100.0);
                    }
                }
            }
        }
        if kind == "success" && reward.is_none() {
            store.append_checkin_log(&format!("wb: 奖励未识别（{name}）响应键: {}",
                dbg_keys.clone().unwrap_or_default()));
        }
        match kind.as_str() {
            "success" => {
                ok += 1;
                if let Some(r) = reward {
                    state.push_credit(CreditRecord {
                        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
                        user_id: aid.clone(),
                        credits: r as i64,
                        delta: r as i64,
                    });
                }
                sink.emit(&Event::account_success(
                    idx, &aid, &name, Some(0), &message, reward.map(|r| r as i64),
                    reward.map(|r| r as i64),
                ));
            }
            "already" => {
                already += 1;
                sink.emit(&Event::account_already(idx, &aid, &name, None));
            }
            _ => {
                failed += 1;
                sink.emit(&Event::account_fail(idx, &aid, &name, &message, None, None, None));
            }
        }
    }

    let _ = store.save_accounts(&accounts_file_mut);
    let _ = store.save_state(&state);
    let now_txt = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    store.append_checkin_log(&format!(
        "[{now_txt}] [wb] 成功{ok}/已签到{already}/失败{failed}/总计{}",
        accounts.len()
    ));
    sink.emit(&Event::done(ok, already, failed));
    Event::done(ok, already, failed).to_json()
}

fn persist_creds(store: &Store, file: &mut crate::store::AccountsFile, uid: &str, creds: &Creds) {
    for a in file.accounts.iter_mut() {
        if a.app == APP_WB && a.user_id == uid {
            a.access_token = Some(creds.access_token.clone());
            a.refresh_token = Some(creds.refresh_token.clone());
            a.access_token_expires_at = creds.expires_at_ms.map(|ms| ms / 1000);
            a.refresh_token_expires_at = creds.refresh_expires_at_ms.map(|ms| ms / 1000);
            a.needs_relogin = false;
            a.relogin_reason.clear();
            a.updated_at = Some(crate::util::now_iso_local());
        }
    }
    let _ = store.save_accounts(file);
}

fn mark_needs_relogin(store: &Store, file: &mut crate::store::AccountsFile, uid: &str, reason: &str) {
    for a in file.accounts.iter_mut() {
        if a.app == APP_WB && a.user_id == uid {
            a.needs_relogin = true;
            a.relogin_reason = reason.to_string();
            a.updated_at = Some(crate::util::now_iso_local());
        }
    }
    let _ = store.save_accounts(file);
    store.append_checkin_log(&format!("wb: 账号 {uid} 已标记需重新登录（{reason}）"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_matches_source() {
        // deterministic: sha256("wb-fingerprint:machine:uid")[:16]
        assert_eq!(
            derive_device_fingerprint("u1", "machine").unwrap(),
            &sha256_hex("wb-fingerprint:machine:u1")[..32]
        );
        assert_eq!(derive_device_fingerprint("", "machine"), None);
    }

    #[test]
    fn region_routing() {
        assert_eq!(region_billing_base(""), BILLING_BASE_CN);
        assert_eq!(region_billing_base("cn.example"), BILLING_BASE_CN);
        assert_eq!(region_billing_base("x.workbuddy.ai"), BILLING_BASE_GLOBAL);
        let [a, b] = billing_bases("x.workbuddy.ai");
        assert_eq!(a, BILLING_BASE_GLOBAL);
        assert_eq!(b, BILLING_BASE_CN);
    }

    #[test]
    fn headers_carry_fingerprints_and_web_platform() {
        let mut creds = Creds::default();
        creds.access_token = "tok".into();
        creds.uid = "u1".into();
        creds.domain = "cn".into();
        let cfg = crate::config::AppConfig::default();
        let h = build_auth_headers(&cfg, &creds, false);
        let get = |k: &str| h.iter().find(|(hk, _)| hk == k).map(|(_, v)| v.clone());
        assert_eq!(get("Authorization").unwrap(), "Bearer tok");
        assert_eq!(get("User-Agent").unwrap(), WB_DESKTOP_UA);
        assert_eq!(get("X-Machine-ID").unwrap(), derive_device_fingerprint("u1", "machine").unwrap());
        assert_eq!(get("X-Session-ID").unwrap(), derive_device_fingerprint("u1", "session").unwrap());
        assert!(get("X-Client-Platform").is_none());
        let h2 = build_auth_headers(&cfg, &creds, true);
        assert_eq!(h2.iter().find(|(k, _)| k == "X-Client-Platform").unwrap().1, "web");
    }

    #[test]
    fn lazy_renew_semantics() {
        let now = 1_700_000_000_000i64;
        // 25h remaining, lazy 24h → skip
        assert!(lazy_renew_skippable(Some(now + 25 * 3_600_000), 24, now));
        // 23h remaining → refresh
        assert!(!lazy_renew_skippable(Some(now + 23 * 3_600_000), 24, now));
        // unknown expiry → not skippable
        assert!(!lazy_renew_skippable(None, 24, now));
        // negative remaining → refresh
        assert!(!lazy_renew_skippable(Some(now - 1000), 24, now));
    }

    #[test]
    fn reward_deep_dig_excludes_balance() {
        // exact-key list hit wins (add_integral is in the caller's candidate list)
        let body = json!({"data": {"result": {"add_integral": 100, "balance": 5000}}});
        assert_eq!(
            num_or_none(dig(&body, &["reward", "credits", "credit", "integral", "add_integral"])),
            Some(100.0)
        );
        // no exact key → deep dig hits a reward-hint key (bonus), never balance/total
        let body2 = json!({"data": {"result": {"bonus": 50, "balance": 5000, "total": 5000}}});
        assert_eq!(deep_reward_dig(&body2, 0), Some(50.0));
        // a body with only balance-like keys yields nothing
        assert_eq!(deep_reward_dig(&json!({"balance": 99, "total": 99}), 0), None);
        // regression (2026-10-18 device log): WB claim returns data.credit (singular)
        let body3 = json!({"code": 0, "data": {"credit": 20, "is_streak_day": 3, "streak_days": 3}, "msg": "ok"});
        assert_eq!(
            num_or_none(dig(&body3, &["reward", "credits", "credit", "integral", "add_integral"])),
            Some(20.0)
        );
        assert_eq!(deep_reward_dig(&body3, 0), Some(20.0));
    }

    #[test]
    fn py_bool_semantics() {
        assert!(!py_truthy(None));
        assert!(!py_truthy(Some(&json!(false))));
        assert!(!py_truthy(Some(&json!(0))));
        assert!(!py_truthy(Some(&json!(""))));
        assert!(py_truthy(Some(&json!(1))));
        assert!(py_truthy(Some(&json!("x"))));
        assert!(py_truthy(Some(&json!({}))));
    }
}
