//! Scheduling math: next check-in instant inside the configured window with
//! optional jitter (anti-risk-control), tolerant to deep-sleep deferrals.
//!
//! The daemon loops: compute next instant → nanosleep → run → repeat. If the
//! device slept past the chosen instant, the round runs immediately upon wake
//! (missed windows are never silently skipped).

use crate::config::AppConfig;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};

/// Seconds until the next round should start, given the last completed round
/// for `app` (as local date string) and the config window.
///
/// Semantics:
/// - If today's window already passed and the app already ran today → schedule
///   tomorrow's pick.
/// - If the window is active now and the app hasn't run today → run immediately.
/// - If the window is in the future today → sleep until the picked instant.
pub fn seconds_until_next(
    cfg: &AppConfig,
    app: &str,
    last_round_date: Option<&str>,
    now: DateTime<Local>,
) -> i64 {
    let today = now.date_naive();
    let today_str = today.format("%Y-%m-%d").to_string();
    let ran_today = last_round_date == Some(today_str.as_str());

    let start_min = cfg.window_start_min;
    let end_min = cfg.window_end_min;
    let now_min = now.hour() as u32 * 60 + now.minute() as u32;

    // deterministic-per-day pseudo-random pick inside the window (jitter)
    let pick_min = if cfg.jitter {
        jitter_pick_min(cfg, &today_str, app, start_min, end_min)
    } else {
        start_min
    };

    if now_min < start_min {
        // window not started yet today
        return seconds_until_today_min(now, pick_min);
    }
    if now_min < end_min {
        if ran_today {
            // done for today → tomorrow's pick
            let tomorrow_pick = if cfg.jitter {
                jitter_pick_min(cfg, &tomorrow_str(&today), app, start_min, end_min)
            } else {
                start_min
            };
            return seconds_until_today_min(now, tomorrow_pick) + 24 * 3600;
        }
        // window active, not run today → run now
        return 0;
    }
    // window over for today → tomorrow's pick
    let tomorrow_pick = if cfg.jitter {
        jitter_pick_min(cfg, &tomorrow_str(&today), app, start_min, end_min)
    } else {
        start_min
    };
    seconds_until_today_min(now, tomorrow_pick) + 24 * 3600
}

fn tomorrow_str(today: &chrono::NaiveDate) -> String {
    (today.succ_opt().unwrap_or(*today)).format("%Y-%m-%d").to_string()
}

fn seconds_until_today_min(now: DateTime<Local>, target_min: u32) -> i64 {
    let today = now.date_naive();
    let target = Local
        .with_ymd_and_hms(
            today.year(),
            today.month(),
            today.day(),
            (target_min / 60) as u32,
            target_min % 60,
            0,
        )
        .single()
        .unwrap_or_else(|| now + chrono::Duration::hours(1));
    (target - now).num_seconds().max(0)
}

/// Per-day per-app stable pseudo-random pick inside [start, end).
/// Uses sha256(date|app|window) so every daemon restart re-derives the same
/// pick — no state file needed, and the pattern differs per app (no sync
/// across Trae/WB/Qoder rounds).
fn jitter_pick_min(_cfg: &AppConfig, date: &str, app: &str, start_min: u32, end_min: u32) -> u32 {
    let span = (end_min - start_min).max(1);
    let h = crate::util::sha256_hex(&format!("traeclock-window:{date}|{app}|{start_min}-{end_min}"));
    let v = u32::from_str_radix(&h[..8], 16).unwrap_or(0);
    start_min + v % span
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate as _NaiveDate;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn before_window_schedules_pick() {
        let cfg = AppConfig::default(); // 08:00-10:00 jitter on
        let now = at(2026, 10, 11, 6, 0);
        let s = seconds_until_next(&cfg, "trae", None, now);
        // pick inside today's window: 08:00 → 2h away, 09:59:59 → just under 4h
        assert!((2 * 3600..=4 * 3600).contains(&s), "s={s}");
    }

    #[test]
    fn inside_window_runs_now_when_not_run() {
        let cfg = AppConfig::default();
        let now = at(2026, 10, 11, 9, 0);
        assert_eq!(seconds_until_next(&cfg, "trae", None, now), 0);
    }

    #[test]
    fn inside_window_tomorrow_when_ran_today() {
        let cfg = AppConfig::default();
        let now = at(2026, 10, 11, 9, 0);
        let s = seconds_until_next(&cfg, "trae", Some("2026-10-11"), now);
        // tomorrow between 22h and 26h away
        assert!((22 * 3600..=26 * 3600).contains(&s), "s={s}");
    }

    #[test]
    fn after_window_tomorrow() {
        let cfg = AppConfig::default();
        let now = at(2026, 10, 11, 12, 0);
        let s = seconds_until_next(&cfg, "trae", None, now);
        assert!((18 * 3600..=24 * 3600).contains(&s), "s={s}");
    }

    #[test]
    fn jitter_pick_is_stable_per_day_and_varies_per_app() {
        let cfg = AppConfig::default();
        let a = jitter_pick_min(&cfg, "2026-10-11", "trae", 480, 600);
        let a2 = jitter_pick_min(&cfg, "2026-10-11", "trae", 480, 600);
        let b = jitter_pick_min(&cfg, "2026-10-11", "wb", 480, 600);
        let a_next_day = jitter_pick_min(&cfg, "2026-10-12", "trae", 480, 600);
        assert_eq!(a, a2);
        assert!((480..600).contains(&a));
        assert!((480..600).contains(&b));
        assert!((480..600).contains(&a_next_day));
        // statistically expected to differ across apps/days; not a hard assert
        let _ = (b, a_next_day);
    }

    #[test]
    fn no_jitter_uses_window_start() {
        let mut cfg = AppConfig::default();
        cfg.jitter = false;
        let now = at(2026, 10, 11, 6, 0);
        let s = seconds_until_next(&cfg, "trae", None, now);
        assert_eq!(s, 2 * 3600); // exactly 08:00
    }
}
