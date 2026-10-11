//! traeclockd — the KernelSU module daemon.
//!
//! Low-power design: single-threaded, no wake locks, no timers beyond a
//! nanosleep main loop. Per app, the next check-in instant is derived from the
//! config window (with per-day jitter); deep sleep defers the timer and the
//! round simply runs at wake. RSS target < 5 MB, CPU ≈ 0 between rounds.

use std::io::Write;
use std::path::PathBuf;

use chrono::Timelike;
use tc_core::config::AppConfig;
use tc_core::events::{Event, Sink};
use tc_core::schedule;
use tc_core::store::{Store, APP_QODER, APP_TRAE, APP_WB};

const DATA_DIR_ENV: &str = "TRAECLOCK_DATA_DIR";
const DEFAULT_DATA_DIR: &str = "/data/adb/traeclock";
/// Max no-network backoff attempts per round trigger.
const NET_RETRY_MAX: u32 = 3;
const PID_FILE: &str = "daemon.pid";

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
    write_pid(&store);

    // single-instance guard: refuse to start if a live daemon PID exists
    if let Some(pid) = read_pid(&store) {
        if pid != std::process::id() && process_alive(pid) {
            eprintln!("traeclockd: another daemon is running (pid {pid}), exiting");
            std::process::exit(0);
        }
    }

    eprintln!("traeclockd: started, data dir {data_dir:?}");
    store.append_checkin_log(&format!(
        "[{}] traeclockd 启动",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));

    loop {
        let cfg = store.config();
        let mut wake = next_wake_secs(&store, &cfg);
        // cap the sleep to 30-minute chunks so config changes take effect promptly
        wake = wake.min(30 * 60).max(60);
        eprintln!("traeclockd: sleeping {wake}s");
        std::thread::sleep(std::time::Duration::from_secs(wake as u64));

        let cfg = store.config();
        let now = chrono::Local::now();
        let now_min = now.hour() * 60 + now.minute();
        let in_window = now_min >= cfg.window_start_min && now_min < cfg.window_end_min;
        if !in_window {
            continue; // config changed or sleep drift; recompute
        }

        // run each app that hasn't completed today (network backoff bounded)
        let mut sink = FileSink { store: store.clone() };
        for app in [APP_TRAE, APP_WB, APP_QODER] {
            let enabled = match app {
                APP_TRAE => cfg.enable_trae,
                APP_WB => cfg.enable_wb,
                _ => cfg.enable_qoder,
            };
            if !enabled || already_ran_today(&store, app) {
                continue;
            }
            let mut attempts = 0u32;
            loop {
                let before = net_probe();
                let result = tc_core::runner::run_one(&store, &cfg, app, &mut sink);
                match result {
                    Some(v) => {
                        let failed = v.get("failed").and_then(|x| x.as_i64()).unwrap_or(0);
                        let ok = v.get("ok").and_then(|x| x.as_i64()).unwrap_or(0)
                            + v.get("already").and_then(|x| x.as_i64()).unwrap_or(0);
                        if failed == 0 || ok > 0 || attempts >= NET_RETRY_MAX - 1 {
                            break;
                        }
                        // all-failed round may be pure network loss → bounded retry
                        if before == false && !net_probe() {
                            attempts += 1;
                            std::thread::sleep(std::time::Duration::from_secs(cfg.backoff_secs));
                            continue;
                        }
                        break;
                    }
                    None => break,
                }
            }
        }
    }
}

/// Seconds until any app's next round (min across apps), derived from schedule.
fn next_wake_secs(store: &Store, cfg: &AppConfig) -> i64 {
    let now = chrono::Local::now();
    let state = store.state();
    let mut best = i64::MAX;
    for app in [APP_TRAE, APP_WB, APP_QODER] {
        let enabled = match app {
            APP_TRAE => cfg.enable_trae,
            APP_WB => cfg.enable_wb,
            _ => cfg.enable_qoder,
        };
        if !enabled {
            continue;
        }
        let last_date = state
            .last_rounds
            .get(app)
            .map(|r| r.time.clone())
            .unwrap_or_default();
        let last_date = if last_date.starts_with(&now.format("%Y-%m-%d").to_string()) {
            Some(now.format("%Y-%m-%d").to_string())
        } else {
            None
        };
        let s = schedule::seconds_until_next(cfg, app, last_date.as_deref(), now);
        best = best.min(s);
    }
    if best == i64::MAX {
        3600
    } else {
        best.max(0)
    }
}

fn already_ran_today(store: &Store, app: &str) -> bool {
    let state = store.state();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    state
        .last_rounds
        .get(app)
        .map(|r| r.time.starts_with(&today))
        .unwrap_or(false)
}

/// Cheap connectivity probe: RFC 2606 test endpoint with a short timeout.
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
    // /proc is available on Android
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
