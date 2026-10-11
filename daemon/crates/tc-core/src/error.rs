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
}
