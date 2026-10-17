//! traeclockd — the KernelSU module daemon.
//!
//! Low-power design: single-threaded, no wake locks, no timers beyond a
//! nanosleep main loop. Per app, the next check-in instant is derived from the
//! config window (with per-day jitter); deep sleep defers the timer and the
//! round simply runs at wake. RSS target < 5 MB, CPU ≈ 0 between rounds.
//!
//! Scheduling guarantee (missed-round recovery): each wake, EVERY enabled app
//! whose daily pick has passed and that has not run today runs IMMEDIATELY —
//! deep sleep pushing us past the window never silently skips a day.

use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Local, Timelike};
use tc_core::config::AppConfig;
use tc_core::events::{Event, Sink};
use tc_core::schedule;
use tc_core::store::{Store, APP_QODER, APP_TRAE, APP_WB};

const DATA_DIR_ENV: &str = "TRAECLOCK_DATA_DIR";
const DEFAULT_DATA_DIR: &str = "/data/adb/traeclock";
/// Max no-network backoff attempts per round trigger.
const NET_RETRY_MAX: u32 = 3;
const PID_FILE: &str = "daemon.pid";

/// INFO logger: every line carries a local timestamp, e.g.
/// `[2026-10-11 16:03:29] [INFO] traeclockd: sleeping 600s`.
struct Log {
    store: Store,
}

impl Log {
    fn info(&self, msg: &str) {
        let line = format!("[{}] [INFO] {}", Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
        eprintln!("{line}");
        self.store.append_checkin_log(&line);
    }
}

struct FileSink {
    store: Store,
}

impl Sink for FileSink {
    fn emit(&mut self, ev: &Event) {
        self.store.append_event(&ev.to_line());
    }
}

fn main() {
    let data_dir = PathBuf::from(
        std::env::var(DATA_DIR_ENV).unwrap_or_else(|_| DEFAULT_DATA_DIR.to_string()),
    );
    let store = Store::new(&data_dir);
    if let Err(e) = store.ensure_dirs() {
        eprintln!("traeclockd: cannot create data dir {data_dir:?}: {e}");
        std::process::exit(1);
    }
    let log = Log { store: store.clone() };

    // single-instance guard: refuse to start if a live daemon PID exists
    write_pid(&store);
    if let Some(pid) = read_pid(&store) {
        if pid != std::process::id() && process_alive(pid) {
            log.info(&format!("another daemon is running (pid {pid}), exiting"));
            std::process::exit(0);
        }
    }

    log.info(&format!(
        "traeclockd started, data dir {}, local TZ {} (offset {}), pid {}",
        data_dir.display(),
        iana_tz_name(),
        Local::now().format("%z"),
        std::process::id()
    ));
    let cfg0 = store.config();
    log.info(&format!(
        "window {:02}:{:02}-{:02}:{:02} jitter={} gap={}s apps(t/w/q)={}/{}/{}",
        cfg0.window_start_min / 60,
        cfg0.window_start_min % 60,
        cfg0.window_end_min / 60,
        cfg0.window_end_min % 60,
        cfg0.jitter,
        cfg0.account_gap_secs,
        cfg0.enable_trae,
        cfg0.enable_wb,
        cfg0.enable_qoder,
    ));

    loop {
        let cfg = store.config();
        // cap the sleep to 10-minute chunks so config changes take effect promptly
        let wake = next_wake_secs(&store, &cfg).clamp(60, 600);
        log.info(&format!("sleeping {wake}s until next scheduled check"));
        std::thread::sleep(std::time::Duration::from_secs(wake as u64));

        let cfg = store.config();
        run_due_apps(&store, &cfg, &log);
    }
}

/// Run every enabled app that is due: due = today's pick has passed AND the
/// app has not completed a round today. This is the missed-round recovery —
/// if deep sleep pushed us past the window, we run immediately on wake.
fn run_due_apps(store: &Store, cfg: &AppConfig, log: &Log) {
    let now = Local::now();
    let now_min = now.hour() * 60 + now.minute();
    for app in [APP_TRAE, APP_WB, APP_QODER] {
        let enabled = match app {
            APP_TRAE => cfg.enable_trae,
            APP_WB => cfg.enable_wb,
            _ => cfg.enable_qoder,
        };
        if !enabled {
            continue;
        }
        let pick_min = schedule::pick_min_for_day(cfg, &now.format("%Y-%m-%d").to_string(), app);
        if now_min < pick_min {
            continue; // today's slot not reached yet
        }
        if already_ran_today(store, app) {
            continue;
        }
        log.info(&format!(
            "round due: {app} (pick {:02}:{:02}, now {:02}:{:02})",
            pick_min / 60,
            pick_min % 60,
            now.hour(),
            now.minute(),
        ));
        run_with_net_backoff(store, cfg, app, log);
    }
}

fn run_with_net_backoff(store: &Store, cfg: &AppConfig, app: &str, log: &Log) {
    let mut sink = FileSink { store: store.clone() };
    for attempt in 0..NET_RETRY_MAX {
        let net_ok = net_probe();
        if !net_ok {
            log.info(&format!(
                "network unreachable before {app} round (attempt {}/{})",
                attempt + 1,
                NET_RETRY_MAX
            ));
        }
        let result = tc_core::runner::run_one(store, cfg, app, &mut sink);
        let Some(v) = result else { return };
        let ok = v.get("ok").and_then(|x| x.as_i64()).unwrap_or(0);
        let already = v.get("already").and_then(|x| x.as_i64()).unwrap_or(0);
        let failed = v.get("failed").and_then(|x| x.as_i64()).unwrap_or(0);
        log.info(&format!("{app} round done: ok={ok} already={already} failed={failed}"));
        // retry only when the whole round failed AND the network is actually down
        if failed == 0 || ok > 0 || already > 0 || net_ok {
            return;
        }
        if attempt + 1 < NET_RETRY_MAX {
            log.info(&format!("retrying {app} in {}s (all failed + no network)", cfg.backoff_secs));
            std::thread::sleep(std::time::Duration::from_secs(cfg.backoff_secs));
        }
    }
}

/// Seconds until the nearest upcoming daily pick (or a due-but-not-run round → 0).
/// The 10-minute cap in main() keeps the daemon responsive to config changes
/// and lets tomorrow's pick be recomputed after midnight.
fn next_wake_secs(store: &Store, cfg: &AppConfig) -> i64 {
    let now: DateTime<Local> = Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let now_min = now.hour() * 60 + now.minute();
    let mut best = 86400i64;

    for app in [APP_TRAE, APP_WB, APP_QODER] {
        let enabled = match app {
            APP_TRAE => cfg.enable_trae,
            APP_WB => cfg.enable_wb,
            _ => cfg.enable_qoder,
        };
        if !enabled {
            continue;
        }
        let pick_min = schedule::pick_min_for_day(cfg, &today, app);
        let ran_today = already_ran_today(store, app);
        if now_min < pick_min {
            // upcoming today
            best = best.min(pick_min as i64 * 60 - now_min as i64 * 60 - now.second() as i64);
        } else if !ran_today {
            // due now (missed-window recovery) → wake immediately
            return 0;
        }
    }
    best.max(0)
}

fn already_ran_today(store: &Store, app: &str) -> bool {
    let state = store.state();
    let today = Local::now().format("%Y-%m-%d").to_string();
    state
        .last_rounds
        .get(app)
        .map(|r| r.time.starts_with(&today))
        .unwrap_or(false)
}

/// Cheap connectivity probe.
fn net_probe() -> bool {
    let agent = tc_core::http::agent(5);
    let (status, _, _) = tc_core::http::get_json(&agent, "https://www.trae.cn/favicon.ico", &[]);
    status != 0
}

fn write_pid(store: &Store) {
    let path = store.data_dir.join(PID_FILE);
    if let Ok(mut f) = std::fs::File::create(&path) {
        let _ = writeln!(f, "{}", std::process::id());
    }
}

fn read_pid(store: &Store) -> Option<u32> {
    std::fs::read_to_string(store.data_dir.join(PID_FILE))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Best-effort timezone name for the startup log (Android: TZ env or
/// persist.sys.timezone property file; fall back to the UTC offset).
fn iana_tz_name() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }
    if let Ok(out) = std::process::Command::new("getprop").arg("persist.sys.timezone").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    format!("unknown({})", Local::now().format("%z"))
}
