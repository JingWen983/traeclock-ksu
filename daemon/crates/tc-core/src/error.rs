//! Error classification + cooldown rules — port of `classify_error` /
//! `save_cooldown` from `tasks/trae_checkin.rs` (plus the source's
//! `humanize_device_limit` device-binding-limit translation, issue #76).
//!
//! Semantics:
//! - cooldown_seconds: -1 = permanent, 0 = record only (no cooldown), >0 = seconds.
//! - Server/Client errors: first 2 occurrences only count, the 3rd starts a cooldown.
//! - Network transport errors (http_status == 0) → Unknown, not persisted.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CooldownEntry {
    pub error_type: String,
    /// unix seconds when cooldown ends; 0 = counting only; 9_999_999_999 = permanent
    pub until: i64,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub error_count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CooldownMap {
    #[serde(default)]
    pub cooldowns: HashMap<String, CooldownEntry>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Classify a check-in error → (error_type, cooldown_seconds).
pub fn classify_error(http_status: i32, code: Option<i64>) -> (&'static str, i64) {
    if http_status == 200 && code == Some(1005) {
        return ("PlanLimit", 43_200);
    }
    if http_status == 429 {
        return ("SoftRate", 60);
    }
    if http_status == 401 {
        return ("SessionDead", -1);
    }
    if http_status == 404 {
        return ("NotFound", 60);
    }
    if (500..600).contains(&http_status) {
        return ("Server", 600);
    }
    if (400..500).contains(&http_status) {
        return ("Client", 600);
    }
    if code.is_some_and(|c| c != 0) {
        return ("BusinessError", 300);
    }
    ("Unknown", 0)
}

/// Translate "device binding limit reached" server errors into actionable text.
/// Exact phrases only — loose "已达上限"/"账户数量" would false-positive on
/// quota errors (locked by upstream test).
pub fn humanize_device_limit(message: &str) -> String {
    let lower = message.to_lowercase();
    let hit = message.contains("绑定的账户数量已达上限")
        || lower.contains("device limit")
        || lower.contains("bound to this device");
    if !hit {
        return message.to_string();
    }
    format!(
        "{message} —— 设备绑定数已达服务端上限：对本账号重新执行一次 OAuth 登录即可自动轮换到其他本机设备凭证，或在 Trae 客户端解绑不再使用的账号"
    )
}

/// 「设备当日已签」特征词判定（upstream issue #110）：仅作用于 claim 失败分支，
/// 成功响应不含「今日已经签到」，误伤面极小；中英文特征词均覆盖。
pub fn is_device_signed_error(message: &str) -> bool {
    message.contains("今日已经签到")
        || message.contains("设备已签到")
        || message.contains("already checked in")
}

/// 识别「设备今日已签到」类服务端业务拒绝并转译为可操作引导（issue #110）：
/// 签到 claim 按设备（x-device-id）每日去重一次，同设备多账号时先签者消耗当日
/// 额度，后签者被拒——status 预检按账号口径仍显示「未签」，用户易误判为工具 bug。
pub fn humanize_device_signed(message: &str) -> String {
    if !is_device_signed_error(message) {
        return message.to_string();
    }
    format!(
        "{message} —— 服务端按设备（而非账号）每日仅计一次签到，本机多个账号共用同一设备凭证时，先签到的账号消耗当日额度：对本账号重新执行一次 OAuth 登录可轮换到其他本机设备凭证，或在另一台设备上签到"
    )
}

/// 「服务端拒绝认证」特征词判定（issue #128）：HTTP 200 + 业务码（如 4014）拒绝的
/// 凭证级认证失败——重试无法恢复，需重走 OAuth。特征词取服务端完整短语（小写比对），
/// 过宽会误伤其他业务错误触发重新登录引导（对齐 #76 窄口径教训）。
pub fn is_auth_rejected_error(message: &str) -> bool {
    message.to_lowercase().contains("not able to authenticate you")
}

/// 识别「服务端拒绝认证」类错误并追加 Tip（issue #128）：命中特征词时追加原因与
/// 临时方案，否则原样透传。
pub fn humanize_auth_rejected(message: &str) -> String {
    if !is_auth_rejected_error(message) {
        return message.to_string();
    }
    format!(
        "{message} —— Tip: 该账号登录凭证被 Trae 服务端拒绝认证（服务端侧认证失败，与工具版本升级无关，自动重试无法恢复）。临时方案：在账号管理对该账号重新执行一次 OAuth 登录，新凭证生效后签到即恢复"
    )
}

/// 签到/状态错误文案统一转译入口：设备绑定上限（#76）+ 设备当日已签（#110）
/// + 服务端拒绝认证（#128）。
pub fn humanize_checkin_error(message: &str) -> String {
    humanize_auth_rejected(&humanize_device_signed(&humanize_device_limit(message)))
}

/// 「设备当日已签」冷却终点：明日 00:05（服务端按自然日去重，跨零点即解除；
/// 留 5 分钟时钟偏移余量）。解析失败回退 12h，下限 10 分钟。
pub fn secs_until_next_checkin_reset() -> i64 {
    use chrono::TimeZone as _;
    let now = chrono::Local::now();
    let next = (now.date_naive() + chrono::Duration::days(1))
        .and_hms_opt(0, 5, 0)
        .and_then(|n| chrono::Local.from_local_datetime(&n).single());
    match next {
        Some(t) => (t - now).num_seconds().max(600),
        None => 43_200,
    }
}

/// In-memory cooldown state with the source's persistence semantics.
/// The daemon persists the map to disk via the store layer; this type holds
/// the pure update logic so tests run without any filesystem.
#[derive(Debug, Default)]
pub struct CooldownState {
    pub map: CooldownMap,
}

impl CooldownState {
    /// Update cooldown for `uid` (port of `save_cooldown`).
    /// cooldown_seconds == 0 → remove entry (success / clear).
    pub fn update(&mut self, uid: &str, error_type: &str, cooldown_seconds: i64, reason: &str) {
        if cooldown_seconds == 0 {
            self.map.cooldowns.remove(uid);
            self.map.updated_at = Some(now_iso());
            return;
        }
        let entry = if cooldown_seconds == -1 {
            CooldownEntry {
                error_type: error_type.into(),
                until: 9_999_999_999,
                reason: reason.into(),
                error_count: 0,
            }
        } else if error_type == "Server" || error_type == "Client" {
            let count = self
                .map
                .cooldowns
                .get(uid)
                .map(|e| e.error_count)
                .unwrap_or(0)
                + 1;
            if count < 3 {
                CooldownEntry {
                    error_type: error_type.into(),
                    until: 0,
                    reason: reason.into(),
                    error_count: count,
                }
            } else {
                CooldownEntry {
                    error_type: error_type.into(),
                    until: chrono::Utc::now().timestamp() + cooldown_seconds,
                    reason: reason.into(),
                    error_count: 0,
                }
            }
        } else {
            CooldownEntry {
                error_type: error_type.into(),
                until: chrono::Utc::now().timestamp() + cooldown_seconds,
                reason: reason.into(),
                error_count: 0,
            }
        };
        self.map.cooldowns.insert(uid.to_string(), entry);
        self.map.updated_at = Some(now_iso());
    }

    /// Is `uid` currently cooling down (or permanently dead)?
    pub fn is_cooling(&self, uid: &str, now_ts: i64) -> bool {
        match self.map.cooldowns.get(uid) {
            Some(e) => e.until > now_ts,
            None => false,
        }
    }

    /// Clear cooldown after a successful check-in.
    pub fn clear(&mut self, uid: &str) {
        self.update(uid, "", 0, "");
    }
}

fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_by_status_and_code() {
        assert_eq!(classify_error(200, Some(1005)), ("PlanLimit", 43_200));
        assert_eq!(classify_error(429, None), ("SoftRate", 60));
        assert_eq!(classify_error(401, None), ("SessionDead", -1));
        assert_eq!(classify_error(404, None), ("NotFound", 60));
        assert_eq!(classify_error(502, None), ("Server", 600));
        assert_eq!(classify_error(403, None), ("Client", 600));
        assert_eq!(classify_error(200, Some(7)), ("BusinessError", 300));
        assert_eq!(classify_error(0, None), ("Unknown", 0));
    }

    #[test]
    fn server_errors_count_twice_then_cool() {
        let mut st = CooldownState::default();
        st.update("u1", "Server", 600, "boom");
        assert!(!st.is_cooling("u1", chrono::Utc::now().timestamp()));
        st.update("u1", "Server", 600, "boom");
        assert!(!st.is_cooling("u1", chrono::Utc::now().timestamp()));
        st.update("u1", "Server", 600, "boom");
        assert!(st.is_cooling("u1", chrono::Utc::now().timestamp()));
        // clear on success
        st.clear("u1");
        assert!(!st.is_cooling("u1", chrono::Utc::now().timestamp()));
    }

    #[test]
    fn permanent_and_immediate_cooldowns() {
        let mut st = CooldownState::default();
        st.update("u2", "SessionDead", -1, "401");
        assert!(st.is_cooling("u2", chrono::Utc::now().timestamp()));
        assert_eq!(st.map.cooldowns["u2"].until, 9_999_999_999);
        st.update("u3", "SoftRate", 60, "429");
        assert!(st.is_cooling("u3", chrono::Utc::now().timestamp()));
    }

    #[test]
    fn device_limit_translation() {
        let m = "该设备绑定的账户数量已达上限。";
        let out = humanize_device_limit(m);
        assert!(out.starts_with(m));
        assert!(out.contains("重新执行一次 OAuth 登录"));
        // loose phrase must NOT hit
        assert_eq!(humanize_device_limit("当前套餐使用额度已达上限"), "当前套餐使用额度已达上限");
        assert_eq!(humanize_device_limit("HTTP 502"), "HTTP 502");
        // english forms
        assert!(humanize_device_limit("Account limit reached for this device (device limit)")
            .contains("设备绑定数已达服务端上限"));
    }

    /// issue #110：设备当日已签转译——命中特征词追加原因与解决路径，其余透传。
    #[test]
    fn device_signed_translation() {
        let m = "当前设备今日已经签到，请明日再来哦~";
        let out = humanize_device_signed(m);
        assert!(out.starts_with(m));
        assert!(out.contains("每日仅计一次签到"));
        assert!(humanize_device_signed("Device already checked in").contains("每日仅计一次签到"));
        // 窄口径：单独「已签到」/「签到成功」不命中
        assert_eq!(humanize_device_signed("已签到"), "已签到");
        assert_eq!(humanize_device_signed("签到成功，获得5积分"), "签到成功，获得5积分");
        assert_eq!(humanize_device_signed(""), "");
    }

    /// issue #128：服务端拒绝认证——命中完整短语追加 Tip，其余透传。
    #[test]
    fn auth_rejected_translation() {
        let server_msg = "We're sorry, but we are not able to authenticate you. If you continue to experience issues, please contact our support team for assistance.";
        let out = humanize_auth_rejected(server_msg);
        assert!(out.starts_with(server_msg));
        assert!(out.contains("重新执行一次 OAuth 登录"));
        // 大小写不敏感
        assert_ne!(
            humanize_auth_rejected("We are NOT ABLE TO AUTHENTICATE YOU now"),
            "We are NOT ABLE TO AUTHENTICATE YOU now"
        );
        // 短语不完整 → 不命中（窄口径）
        assert!(!is_auth_rejected_error("failed to authenticate, please retry"));
        assert!(!is_auth_rejected_error("not able to authenticate"));
        assert!(!is_auth_rejected_error(""));
    }

    /// 统一转译入口：三级特征词串联，互不回归。
    #[test]
    fn checkin_error_translation_chains_all_tiers() {
        assert!(humanize_checkin_error("We're sorry, but we are not able to authenticate you.")
            .contains("Tip:"));
        assert!(humanize_checkin_error("该设备绑定的账户数量已达上限。").contains("设备绑定数已达服务端上限"));
        assert!(humanize_checkin_error("当前设备今日已经签到，请明日再来哦~").contains("每日仅计一次签到"));
        assert_eq!(humanize_checkin_error("HTTP 502"), "HTTP 502");
    }

    /// 设备当日已签的冷却终点落在明日 00:05 附近（下限 10 分钟保护）。
    #[test]
    fn next_checkin_reset_is_about_next_midnight() {
        let s = secs_until_next_checkin_reset();
        assert!((600..=86_700).contains(&s), "secs={s}");
        assert!(s > 86_400 || s >= 600);
    }
}
