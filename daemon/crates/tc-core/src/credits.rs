//! Real-time credits queries for the manager dashboard — port of the source's
//! three credit paths:
//! - Trae: `POST /trae/api/v2/pay/ide_user_ent_usage` (`require_usage:true`,
//!   `req_source:2`) → user_entitlement_pack_list[]; remaining = quota.credits_limit
//!   - usage.credits_amount, per-pack expire 4102444800 sentinel = "长期有效";
//!   kind: product_id==209 → "Work", else "通用".
//! - WorkBuddy: `POST {region}/billing/meter/get-user-resource-summary`
//!   (+paid/free packages with PackageCodes) → balance = top-level Remaining/Balance
//!   keys else Σ CycleRemainCapacity; packages with expire chains.
//! - Qoder: `GET {openapi}/sash/api/v2/me/usage` → qoderUsage.userQuota/addOnQuota
//!   pairs (remaining / total-used clamp-0), plan expires, total fallback dig.
//! Red line: tokens never appear in outputs.
//!
//! NOTE: responses are parsed leniently and returned as JSON so the manager
//! renders without re-networking; the daemon logs per-app failures.

use serde_json::{json, Value};

use crate::config::AppConfig;
use crate::envelope::dig;
use crate::fingerprint;
use crate::http;
use crate::jwt;
use crate::store::{Account, Store, APP_QODER, APP_TRAE, APP_WB};
use crate::trae;

/// Sentinel expire (2100-01-01) meaning "never expires" (source convention).
pub const NEVER_EXPIRE_TS: i64 = 4102444800;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CreditRow {
    pub app: String,
    pub user_id: String,
    pub name: String,
    pub ok: bool,
    pub message: String,
    /// Balance for WB (billing meter) / total for Trae & Qoder.
    pub balance: Option<f64>,
    /// Trae: 通用 pack sum; Qoder: subscription (plan) remaining.
    pub general: Option<f64>,
    /// Trae: Work pack sum; Qoder: addOn (resource package) remaining.
    pub work: Option<f64>,
    /// Used portion this cycle (Qoder plan.used).
    pub used: Option<f64>,
    /// Earliest expiry (unix seconds; NEVER_EXPIRE_TS when a pack is permanent).
    pub earliest_expire: Option<i64>,
    /// Per-pack detail for the dashboard list.
    pub packs: Vec<Value>,
}

impl CreditRow {
    fn empty(app: &str, user_id: &str, name: &str) -> CreditRow {
        CreditRow {
            app: app.into(),
            user_id: user_id.into(),
            name: name.into(),
            ok: false,
            message: String::new(),
            balance: None,
            general: None,
            work: None,
            used: None,
            earliest_expire: None,
            packs: vec![],
        }
    }
}

fn r2(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

// ── Trae ─────────────────────────────────────────────────────────────────────

/// Trae credits via ide_user_ent_usage (port of query_ent_packs + pack_to_detail
/// + classify_source; kind by product_id==209 → Work else 通用).
pub fn trae_credits(agent: &ureq::Agent, cfg: &AppConfig, acc: &Account) -> CreditRow {
    let name = if acc.name.is_empty() { acc.user_id.clone() } else { acc.name.clone() };
    let mut row = CreditRow::empty(APP_TRAE, &acc.user_id, &name);
    let Some(jwt) = acc.jwt.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        row.message = "未配置 JWT".into();
        return row;
    };
    let uid = match jwt::parse(&jwt).user_id {
        Some(u) => u,
        None => {
            row.message = "无法从 JWT 解析 user id".into();
            return row;
        }
    };
    let dev = trae::device_for(cfg, &uid);
    let url = "https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage";
    let headers = trae::build_headers(cfg, &jwt, &dev);
    let (status, body, raw) = http::post_json(agent, url, &headers, &json!({
        "require_usage": true, "req_source": 2
    }));
    if status != 200 {
        row.message = format!("HTTP {status}: {}", short(&raw));
        return row;
    }
    let Some(b) = body else {
        row.message = "响应非 JSON".into();
        return row;
    };
    let Some(packs) = dig(&b, &["user_entitlement_pack_list"]).and_then(|v| v.as_array()).cloned() else {
        row.message = "响应缺少 user_entitlement_pack_list".into();
        return row;
    };
    let now_ts = chrono::Utc::now().timestamp();
    let mut total = 0.0f64;
    let mut general = 0.0f64;
    let mut work = 0.0f64;
    let mut earliest: Option<i64> = None;
    for pack in &packs {
        let Some(limit) = dig(pack, &["entitlement_base_info", "quota", "credits_limit"]).and_then(|v| v.as_f64()) else {
            continue;
        };
        let used = pack.get("usage").and_then(|u| u.get("credits_amount")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let remaining = (limit - used).max(0.0);
        if remaining <= 0.0 {
            continue;
        }
        let expire = pack.get("expire_time").and_then(|v| v.as_i64()).unwrap_or(NEVER_EXPIRE_TS);
        if expire <= now_ts {
            continue;
        }
        let product_id = dig(pack, &["entitlement_base_info", "product_id"]).and_then(|v| v.as_i64()).unwrap_or(0);
        let kind = if product_id == 209 { "Work" } else { "通用" };
        total += remaining;
        match kind {
            "Work" => work += remaining,
            _ => general += remaining,
        }
        earliest = Some(earliest.map_or(expire, |e: i64| e.min(expire)));
        row.packs.push(json!({
            "kind": kind,
            "source": classify_source(pack),
            "remaining": r2(remaining),
            "total": r2(limit),
            "expire_time": expire,
        }));
    }
    row.ok = true;
    row.balance = Some(r2(total));
    row.general = Some(r2(general));
    row.work = Some(r2(work));
    row.earliest_expire = earliest;
    row
}

/// Source label for a pack (port of classify_source, priority order).
fn classify_source(pack: &Value) -> String {
    let charge = dig(pack, &["entitlement_base_info", "charge_amount"]).and_then(|v| v.as_i64()).unwrap_or(0);
    let group_name = pack.get("group_name").and_then(|v| v.as_str()).unwrap_or("");
    let display_desc = pack.get("display_desc").and_then(|v| v.as_str()).unwrap_or("");
    let combined = format!("{group_name}{display_desc}");
    if charge > 0 {
        return "会员/购买".into();
    }
    if combined.contains("签到") {
        return "每日签到".into();
    }
    if combined.contains("登录") {
        return "每月登录".into();
    }
    if combined.contains("兑换") || combined.to_lowercase().contains("redeem") {
        return "兑换".into();
    }
    if !group_name.is_empty() {
        return group_name.into();
    }
    if !display_desc.is_empty() {
        return display_desc.into();
    }
    "积分包".into()
}

// ── WorkBuddy ────────────────────────────────────────────────────────────────

/// WB balance + packages via billing meter summary (port of balance_from_summary
/// + packages_from chains, web_platform headers as the source uses).
pub fn wb_credits(agent: &ureq::Agent, cfg: &AppConfig, acc: &Account) -> CreditRow {
    let name = if acc.name.is_empty() { acc.user_id.clone() } else { acc.name.clone() };
    let mut row = CreditRow::empty(APP_WB, &acc.user_id, &name);
    let Some(token) = acc.access_token.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        row.message = "无 access_token".into();
        return row;
    };
    let domain = acc.domain.clone().unwrap_or_default();
    let base = crate::workbuddy::region_billing_base(&domain);
    let creds = crate::workbuddy::Creds {
        access_token: token,
        uid: acc.user_id.clone(),
        domain,
        ..Default::default()
    };
    let headers = crate::workbuddy::build_auth_headers(cfg, &creds, true);
    let url = format!("{base}/billing/meter/get-user-resource-summary");
    let (status, body, raw) = http::post_json(agent, &url, &headers, &json!({}));
    if status == 401 {
        row.message = "登录态失效（401）".into();
        return row;
    }
    if status != 200 {
        row.message = format!("HTTP {status}: {}", short(&raw));
        return row;
    }
    let Some(b) = body else {
        row.message = "响应非 JSON".into();
        return row;
    };
    // balance: top-level keys first (num_of tolerates numeric strings — WB
    // returns "3371.00"-style strings), else Σ per-package remaining
    let balance = num_of(dig(&b, &["RemainingCapacity", "remaining", "TotalRemaining", "Balance", "balance"]))
        .or_else(|| sum_cycle_remain(&b));
    // packages: walk dicts with remaining/total semantics
    let mut dicts = Vec::new();
    walk_dicts(&b, 0, &mut dicts);
    let mut packs: Vec<Value> = Vec::new();
    for item in dicts {
        let total = num_of(dig(item, &["CapacitySize", "CycleCapacitySize", "total", "Total"]));
        let remaining = num_of(dig(item, &["CycleRemainCapacity", "RemainingCapacity", "remaining"]));
        if remaining.is_none() && total.is_none() {
            continue;
        }
        let pname = dig(item, &["PackageName", "packageName", "Name", "name", "description"])
            .and_then(|v| match v { Value::String(s) => Some(s.clone()), _ => None })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "积分包".into());
        let expire_raw = dig(item, &["DeductionEndTime", "deductionEndTime", "PackageEndTime", "EndTime", "endTime", "expireTime", "expireAt"]);
        let expire_ts = to_ts(expire_raw).unwrap_or(NEVER_EXPIRE_TS);
        packs.push(json!({
            "kind": "通用",
            "source": pname,
            "remaining": remaining.unwrap_or(0.0),
            "total": total.unwrap_or(0.0),
            "expire_time": expire_ts,
        }));
    }
    row.ok = balance.is_some() || !packs.is_empty();
    row.message = if row.ok { String::new() } else { "响应结构未识别".into() };
    // Dashboard consistency: when per-package rows parsed, balance = Σ packs
    // (the top-level `remaining` key can match non-credit rate-limit fields
    // and return a bogus 0 while real packs exist — screenshot-verified bug).
    let pack_sum: f64 = packs.iter().filter_map(|p| p.get("remaining").and_then(|v| v.as_f64())).sum();
    row.balance = if packs.is_empty() { balance } else { Some(pack_sum.max(balance.unwrap_or(0.0))) }.map(r2);
    row.earliest_expire = packs.iter().filter_map(|p| p.get("expire_time").and_then(|v| v.as_i64())).min();
    row.packs = packs;
    row
}

fn sum_cycle_remain(body: &Value) -> Option<f64> {
    let mut dicts = Vec::new();
    walk_dicts(body, 0, &mut dicts);
    let mut s = 0.0;
    let mut has = false;
    for item in dicts {
        if let Some(v) = num_of(dig(item, &["CycleRemainCapacity", "RemainingCapacity", "remaining"])) {
            s += v;
            has = true;
        }
    }
    has.then_some(s)
}

fn walk_dicts<'a>(v: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
    if depth > 8 || out.len() >= 200 {
        return;
    }
    match v {
        Value::Object(m) => {
            out.push(v);
            for val in m.values() {
                walk_dicts(val, depth + 1, out);
            }
        }
        Value::Array(a) => {
            for x in a {
                walk_dicts(x, depth + 1, out);
            }
        }
        _ => {}
    }
}

fn num_of(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().or_else(|| v?.as_str()?.trim().parse().ok())
}

/// WB expire strings → unix seconds (tolerant: ts / yyyy-MM-dd / RFC3339).
fn to_ts(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(n) = v.as_i64() {
        return Some(if n > 10_000_000_000 { n / 1000 } else { n });
    }
    let s = v.as_str()?.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Some(if n > 10_000_000_000 { n / 1000 } else { n });
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
    }
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

// ── Qoder ────────────────────────────────────────────────────────────────────

/// Qoder balance via sash usage (port of qoder_credits::parse_usage core).
pub fn qoder_credits(agent: &ureq::Agent, cfg: &AppConfig, acc: &Account) -> CreditRow {
    let name = if acc.name.is_empty() { acc.user_id.clone() } else { acc.name.clone() };
    let mut row = CreditRow::empty(APP_QODER, &acc.user_id, &name);
    let creds = crate::qoder::QoderCreds::from_value(acc.qoder_creds.as_ref());
    if creds.access_token.is_empty() {
        row.message = "无可用凭证".into();
        return row;
    }
    let headers = crate::qoder::build_auth_headers(cfg, &creds);
    let url = format!("{}/sash/api/v2/me/usage", crate::qoder::OPEN_API_BASE);
    let (status, body, raw) = http::get_json(agent, &url, &headers);
    if status != 200 {
        row.message = format!("usage 不可用（HTTP {status}）: {}", short(&raw));
        return row;
    }
    let Some(b) = body else {
        row.message = "usage 响应非 JSON".into();
        return row;
    };
    let q = b.get("qoderUsage");
    let (plan, plan_used) = quota_pair(q.and_then(|v| v.get("userQuota")));
    let (addon, addon_used) = quota_pair(q.and_then(|v| v.get("addOnQuota")));
    let plan_expires_at = q
        .and_then(|v| v.get("expiresAt"))
        .and_then(Value::as_i64)
        .map(|ms| {
            chrono::DateTime::from_timestamp_millis(ms)
                .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let total = match (plan, addon) {
        (Some(p), Some(a)) => Some(p + a),
        (Some(p), None) | (None, Some(p)) => Some(p),
        _ => dig(&b, &["totalRemaining", "remainingCredits", "remaining", "balance"]).and_then(|v| v.as_f64()),
    };
    if plan.is_none() && addon.is_none() && total.is_none() {
        row.message = "usage 结构未识别（接口可能已变更）".into();
        return row;
    }
    // addOn package → dashboard entry with the qoderUsage-level expiry date
    if let Some(a) = addon {
        row.packs.push(json!({
            "kind": "资源包",
            "source": "addon",
            "remaining": a,
            "total": Value::Null,
            "expire_time": NEVER_EXPIRE_TS,
            "expire_text": plan_expires_at,
        }));
    }
    row.ok = true;
    row.balance = total.map(r2);
    row.general = plan.map(r2);
    row.work = addon.map(r2);
    row.used = plan_used.map(r2);
    row
}

/// quota 子对象 → (remaining, used)；remaining 缺失时 total-used 推导并钳 0
/// （防服务端脏数据的「负积分」）。
fn quota_pair(q: Option<&Value>) -> (Option<f64>, Option<f64>) {
    let Some(q) = q else { return (None, None) };
    let used = q.get("used").and_then(|v| v.as_f64());
    let remaining = q
        .get("remaining")
        .and_then(|v| v.as_f64())
        .or_else(|| q.get("total").and_then(|v| v.as_f64()).map(|t| (t - used.unwrap_or(0.0)).max(0.0)));
    (remaining, used)
}

fn short(s: &str) -> String {
    s.chars().take(120).collect()
}

// ── aggregate ────────────────────────────────────────────────────────────────

/// Query all enabled accounts (per app) and return rows in app order.
/// One account's failure never blocks the others.
pub fn refresh_all(store: &Store, cfg: &AppConfig) -> Value {
    let agent = http::agent(30);
    let acc = store.accounts();
    let mut rows: Vec<CreditRow> = Vec::new();
    for a in &acc.accounts {
        if !a.enabled {
            continue;
        }
        let row = match a.app.as_str() {
            APP_TRAE => trae_credits(&agent, cfg, a),
            APP_WB => wb_credits(&agent, cfg, a),
            APP_QODER => qoder_credits(&agent, cfg, a),
            _ => continue,
        };
        rows.push(row);
    }
    let total: f64 = rows.iter().filter_map(|r| r.balance).sum();
    json!({
        "ok": true,
        "total": r2(total),
        "fetched_at": crate::util::now_iso_local(),
        "rows": rows,
    })
}

// silence unused import when the fingerprint re-export is not referenced on all platforms
#[allow(unused_imports)]
use fingerprint as _fp_keep;
