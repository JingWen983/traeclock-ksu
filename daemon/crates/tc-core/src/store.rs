//! JSON-file persistence layer (SQLite replacement, atomic tmp+rename writes).
//!
//! Data directory layout (`/data/adb/traeclock/` on device):
//!   config.json          — AppConfig
//!   accounts.json        — AccountsFile (all apps' accounts with credentials)
//!   cooldowns.json       — CooldownMap
//!   state.json           — RunState (last round results, credits history rollup)
//!   logs/checkin.log     — human-readable summary lines
//!   events.ndjson        — machine-readable event stream (rotated)

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
use crate::error::CooldownMap;

// ── account model (union across the three apps) ─────────────────────────────

pub const APP_TRAE: &str = "trae";
pub const APP_WB: &str = "wb";
pub const APP_QODER: &str = "qoder";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    /// App id: trae | wb | qoder.
    pub app: String,
    /// Canonical user id (uid from JWT / server info).
    pub user_id: String,
    /// Display name.
    pub name: String,
    /// Group label (organizational only).
    pub group: Option<String>,
    /// Enabled accounts participate in rounds.
    pub enabled: bool,
    // ── Trae ──
    /// Trae access JWT (Cloud-IDE-JWT token, without prefix).
    pub jwt: Option<String>,
    /// Trae refresh token (OAuth).
    pub refresh_token: Option<String>,
    pub refresh_token_expires_at: Option<i64>,
    pub refresh_token_fails: i64,
    pub refresh_token_invalid: bool,
    // ── WorkBuddy / CodeBuddy ──
    pub access_token: Option<String>,
    pub access_token_expires_at: Option<i64>,
    pub domain: Option<String>,
    pub needs_relogin: bool,
    pub relogin_reason: String,
    // ── Qoder ──
    /// QoderCreds serialized (access_token / machine_id / machine_token / …).
    pub qoder_creds: Option<serde_json::Value>,
    /// Qoder device profile (stable per-account binding).
    pub qoder_device_profile: Option<serde_json::Value>,
    // ── shared lifecycle ──
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountsFile {
    pub accounts: Vec<Account>,
    pub updated_at: Option<String>,
}

// ── run state ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoundRecord {
    pub time: String,
    pub app: String,
    pub ok: usize,
    pub already: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CreditRecord {
    pub date: String,
    pub user_id: String,
    pub credits: i64,
    pub delta: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunState {
    /// Last round per app (trae/wb/qoder).
    pub last_rounds: HashMap<String, RoundRecord>,
    /// Rolling credits history (90 days), one record per account per day.
    pub credits_history: Vec<CreditRecord>,
}

impl RunState {
    /// Append a credit record; keeps the latest per (date, user_id); prunes >90 days.
    pub fn push_credit(&mut self, rec: CreditRecord) {
        self.credits_history
            .retain(|r| !(r.date == rec.date && r.user_id == rec.user_id));
        self.credits_history.push(rec);
        let cutoff = (chrono::Local::now().date_naive() - chrono::Duration::days(90))
            .format("%Y-%m-%d")
            .to_string();
        self.credits_history.retain(|r| r.date >= cutoff);
    }
}

// ── store ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Store {
    pub data_dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store { data_dir: data_dir.to_path_buf() }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.data_dir.join("logs"))?;
        Ok(())
    }

    fn read_json<T: DeserializeOwned>(&self, rel: &str) -> Option<T> {
        let raw = std::fs::read_to_string(self.data_dir.join(rel)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Public typed read for helper files (pending login sessions etc.).
    pub fn read_json_opt<T: DeserializeOwned>(&self, rel: &str) -> Option<T> {
        self.read_json::<T>(rel)
    }

    fn write_json(&self, rel: &str, value: &impl Serialize) -> Result<(), String> {
        let path = self.data_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
        {
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn config(&self) -> AppConfig {
        self.read_json::<AppConfig>("config.json").unwrap_or_default().sanitized()
    }

    pub fn save_config(&self, cfg: &AppConfig) -> Result<(), String> {
        self.write_json("config.json", cfg)
    }

    /// Persist an arbitrary helper JSON file (pending login sessions etc.).
    pub fn save_json(&self, rel: &str, value: &impl Serialize) -> Result<(), String> {
        self.write_json(rel, value)
    }

    pub fn accounts(&self) -> AccountsFile {
        self.read_json::<AccountsFile>("accounts.json").unwrap_or_default()
    }

    pub fn save_accounts(&self, acc: &AccountsFile) -> Result<(), String> {
        let mut acc = acc.clone();
        acc.updated_at = Some(crate::util::now_iso_local());
        self.write_json("accounts.json", &acc)
    }

    pub fn cooldowns(&self) -> CooldownMap {
        self.read_json::<CooldownMap>("cooldowns.json").unwrap_or_default()
    }

    pub fn save_cooldowns(&self, cd: &CooldownMap) -> Result<(), String> {
        self.write_json("cooldowns.json", cd)
    }

    pub fn state(&self) -> RunState {
        self.read_json::<RunState>("state.json").unwrap_or_default()
    }

    pub fn save_state(&self, st: &RunState) -> Result<(), String> {
        self.write_json("state.json", st)
    }

    /// Append a summary line to logs/checkin.log.
    pub fn append_checkin_log(&self, line: &str) {
        let path = self.data_dir.join("logs").join("checkin.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Append an NDJSON event line to logs/events.ndjson (with size rotation ~2MB).
    pub fn append_event(&self, line: &str) {
        let path = self.data_dir.join("logs").join("events.ndjson");
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 2_000_000 {
                let _ = std::fs::rename(&path, self.data_dir.join("logs").join("events.ndjson.1"));
            }
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Read the last N lines of logs/events.ndjson (for the manager app).
    pub fn tail_events(&self, n: usize) -> Vec<String> {
        let path = self.data_dir.join("logs").join("events.ndjson");
        let Ok(raw) = std::fs::read_to_string(&path) else { return Vec::new() };
        let lines: Vec<&str> = raw.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].iter().map(|s| s.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::fingerprint::DeviceEntry;

    fn tmpdir() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!(
            "tc-core-test-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis(),
            n
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrip_config_accounts_state() {
        let dir = tmpdir();
        let st = Store::new(&dir);
        st.ensure_dirs().unwrap();

        let mut cfg = AppConfig::default();
        cfg.header_overrides.insert("trae:x-test".into(), "v".into());
        st.save_config(&cfg).unwrap();
        assert_eq!(st.config().header_overrides["trae:x-test"], "v");

        let mut acc = AccountsFile::default();
        acc.accounts.push(Account {
            app: APP_TRAE.into(),
            user_id: "u1".into(),
            name: "n1".into(),
            enabled: true,
            jwt: Some("abc".into()),
            ..Default::default()
        });
        st.save_accounts(&acc).unwrap();
        let loaded = st.accounts();
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].jwt.as_deref(), Some("abc"));
        assert!(loaded.updated_at.is_some());

        let mut state = RunState::default();
        state.push_credit(CreditRecord {
            date: "2026-10-11".into(),
            user_id: "u1".into(),
            credits: 100,
            delta: 5,
        });
        state.push_credit(CreditRecord {
            date: "2026-10-11".into(),
            user_id: "u1".into(),
            credits: 105,
            delta: 5,
        });
        st.save_state(&state).unwrap();
        assert_eq!(st.state().credits_history.len(), 1); // same-day dedup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn device_map_persists_and_wins_over_derivation() {
        let dir = tmpdir();
        let st = Store::new(&dir);
        let mut cfg = AppConfig::default();
        cfg.device_map.insert(
            "u9".into(),
            DeviceEntry { device_id: "123456789012345".into(), ..Default::default() },
        );
        st.save_config(&cfg).unwrap();
        let cfg2 = st.config();
        assert_eq!(cfg2.device_map["u9"].device_id, "123456789012345");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn events_rotate_and_tail() {
        let dir = tmpdir();
        let st = Store::new(&dir);
        st.ensure_dirs().unwrap();
        for i in 0..50 {
            st.append_event(&format!("{{\"i\":{i}}}"));
        }
        let tail = st.tail_events(5);
        assert_eq!(tail.len(), 5);
        assert!(tail[4].contains("\"i\":49"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
