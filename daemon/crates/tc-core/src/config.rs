//! Runtime configuration (`/data/adb/traeclock/config.json` on device).
//!
//! Anti-risk-control knobs live here: check-in window + jitter, account gap,
//! header overrides (per-app escape hatch), and the platform flavor switch
//! (`windows` default — verbatim desktop header set; `android` mobile hatch).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_WINDOW_START_MIN: u32 = 8 * 60; // 08:00
pub const DEFAULT_WINDOW_END_MIN: u32 = 10 * 60; // 10:00
pub const DEFAULT_ACCOUNT_GAP_SECS: u64 = 3;
pub const DEFAULT_RETRY: u32 = 3;
pub const DEFAULT_BACKOFF_SECS: u64 = 900; // 15 min no-network retry

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Check-in window start, minutes from midnight (local time).
    pub window_start_min: u32,
    /// Check-in window end, minutes from midnight (local time).
    pub window_end_min: u32,
    /// Pick a random instant inside the window each day (anti-risk-control).
    pub jitter: bool,
    /// Serial gap between accounts within a round (seconds).
    pub account_gap_secs: u64,
    /// Network-level retries for claim requests.
    pub retry: u32,
    /// Backoff when the network is unreachable (seconds), max 3 attempts/round.
    pub backoff_secs: u64,
    /// Per-app switches.
    pub enable_trae: bool,
    pub enable_wb: bool,
    pub enable_qoder: bool,
    /// "windows" (default — verbatim desktop header set; new JWTs are issued
    /// with device-fingerprint validation that rejects the android trio) or
    /// "android" (mobile escape hatch).
    pub platform_flavor: String,
    /// Header overrides: "app:Header-Name" → value. Applied last, wins over defaults.
    pub header_overrides: HashMap<String, String>,
    /// Trae device fingerprints: uid → entry (drift guard; wins over re-derivation).
    pub device_map: HashMap<String, crate::fingerprint::DeviceEntry>,
    /// Schema marker for one-time migrations. IMPORTANT: serde default here
    /// is 0 (absent field = pre-0.4.3 config), NOT the struct Default's
    /// CURRENT_SCHEMA — `#[serde(default)]` on the struct routes missing
    /// fields through Default::default(), so this field is excluded from
    /// struct-level default and re-defaulted to 0 via this attribute.
    #[serde(default = "zero_schema")]
    pub schema: u32,
}

fn zero_schema() -> u32 {
    0
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            window_start_min: DEFAULT_WINDOW_START_MIN,
            window_end_min: DEFAULT_WINDOW_END_MIN,
            jitter: true,
            account_gap_secs: DEFAULT_ACCOUNT_GAP_SECS,
            retry: DEFAULT_RETRY,
            backoff_secs: DEFAULT_BACKOFF_SECS,
            enable_trae: true,
            enable_wb: true,
            enable_qoder: true,
            platform_flavor: "windows".into(),
            header_overrides: HashMap::new(),
            device_map: HashMap::new(),
            schema: CURRENT_SCHEMA,
        }
    }
}

/// Bump when a migration is added. 1 = 0.4.3 flavor-default flip.
pub const CURRENT_SCHEMA: u32 = 1;

impl AppConfig {
    /// Validate/repair window bounds, run one-time migrations, then clamp the
    /// platform flavor. Migration (schema 0 → 1): configs persisted while
    /// "android" was the factory default carried that value forward through
    /// upgrades — but the /pay/ credits endpoints reject the android trio
    /// (device-validated JWTs), so upgrade those stale defaults to "windows".
    /// A user who deliberately chose android after 0.4.3 has schema=1 and is
    /// never touched.
    pub fn sanitized(mut self) -> Self {
        if self.window_start_min >= 24 * 60 {
            self.window_start_min = DEFAULT_WINDOW_START_MIN;
        }
        if self.window_end_min > 24 * 60 || self.window_end_min <= self.window_start_min {
            self.window_end_min = DEFAULT_WINDOW_END_MIN;
        }
        if self.schema < 1 && self.platform_flavor == "android" {
            self.platform_flavor = "windows".into();
        }
        self.schema = self.schema.max(CURRENT_SCHEMA);
        if !matches!(self.platform_flavor.as_str(), "android" | "windows") {
            self.platform_flavor = "windows".into();
        }
        self
    }

    /// Apply `app:Header` overrides onto a header list (case-insensitive match).
    pub fn apply_overrides(&self, app: &str, headers: &mut Vec<(String, String)>) {
        let prefix = format!("{app}:");
        for (k, v) in &self.header_overrides {
            let Some(name) = k.strip_prefix(&prefix) else { continue };
            if let Some(slot) = headers
                .iter_mut()
                .find(|(hk, _)| hk.eq_ignore_ascii_case(name))
            {
                slot.1 = v.clone();
            } else {
                headers.push((name.to_string(), v.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_sanitization() {
        let cfg: AppConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(cfg.window_start_min, DEFAULT_WINDOW_START_MIN);
        assert!(cfg.jitter);
        assert_eq!(cfg.platform_flavor, "windows");
        let bad: AppConfig = serde_json::from_value(json!({
            "window_start_min": 9999, "window_end_min": 5, "platform_flavor": "ios"
        }))
        .unwrap();
        let fixed = bad.sanitized();
        assert_eq!(fixed.window_start_min, DEFAULT_WINDOW_START_MIN);
        assert_eq!(fixed.window_end_min, DEFAULT_WINDOW_END_MIN);
        assert_eq!(fixed.platform_flavor, "windows");
    }

    /// schema-0 configs (persisted while android was the factory default)
    /// migrate android → windows; post-0.4.3 deliberate android choices
    /// (schema=1) survive untouched.
    #[test]
    fn schema_migration_flips_stale_android_default() {
        let old = json!({"platform_flavor": "android", "schema": 0}); // explicit pre-0.4.3 marker
        let cfg: AppConfig = serde_json::from_value(old).unwrap();
        let migrated = cfg.sanitized();
        assert_eq!(migrated.platform_flavor, "windows");
        assert_eq!(migrated.schema, CURRENT_SCHEMA);
        // deliberate choice after 0.4.3: schema=1 stays android
        let kept = serde_json::from_value::<AppConfig>(json!({
            "platform_flavor": "android", "schema": 1
        }))
        .unwrap()
        .sanitized();
        assert_eq!(kept.platform_flavor, "android");
        // old config already on windows: unchanged
        let win = serde_json::from_value::<AppConfig>(json!({"platform_flavor": "windows"}))
            .unwrap()
            .sanitized();
        assert_eq!(win.platform_flavor, "windows");
    }

    #[test]
    fn overrides_apply_and_add() {
        let mut cfg = AppConfig::default();
        cfg.header_overrides
            .insert("trae:x-lscbd-platform".into(), "windows".into());
        cfg.header_overrides.insert("trae:x-new-header".into(), "1".into());
        let mut h = vec![
            ("x-lscbd-platform".to_string(), "android".to_string()),
            ("x-user-region".to_string(), "CN".to_string()),
        ];
        cfg.apply_overrides("trae", &mut h);
        assert_eq!(h[0], ("x-lscbd-platform".to_string(), "windows".to_string()));
        assert!(h.iter().any(|(k, v)| k == "x-new-header" && v == "1"));
        // other app untouched
        let mut h2 = vec![("x-lscbd-platform".to_string(), "android".to_string())];
        cfg.apply_overrides("wb", &mut h2);
        assert_eq!(h2[0].1, "android");
    }
}
