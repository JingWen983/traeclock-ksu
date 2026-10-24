//! traeclockctl — management CLI. All subcommands print a single JSON object
//! on the last line (the manager app parses this via `su -c`).
//!
//! Subcommands:
//!   status                      — daemon/config/next-run overview
//!   accounts [app]              — list accounts (credentials masked)
//!   add <app> <json>            — upsert one account from a JSON blob
//!   remove <app> <user_id>      — remove one account
//!   enable/disable <app> <uid>  — toggle an account
//!   run-now [app]               — run a round immediately (skips window wait)
//!   config [json]               — get (no arg) or merge-set config
//!   log [n]                     — last n NDJSON events
//!   checkin-log                 — human summary log
//!   trae-login-url              — start a Trae OAuth session → {url, state}
//!   trae-callback <url>         — finish a Trae OAuth login from the callback
//!   trae-refresh <uid>          — force refresh one Trae account
//!   wb-start <name?>            — WorkBuddy OAuth: state → {state, auth_url}
//!   wb-poll <state>             — WorkBuddy OAuth: poll once → token if done
//!   qoder-start [compat]        — Qoder device flow start → {auth_url, poll}
//!   qoder-poll                  — Qoder device flow poll once
//!   qoder-wait <timeout_s>      — Qoder device flow poll-to-completion loop

use std::io::Write;
use std::path::PathBuf;

use tc_core::config::AppConfig;
use tc_core::events::{Event, Sink};
use tc_core::store::{Account, AccountsFile, Store, APP_QODER, APP_TRAE, APP_WB};

const DATA_DIR_ENV: &str = "TRAECLOCK_DATA_DIR";
const DEFAULT_DATA_DIR: &str = "/data/adb/traeclock";

struct StdoutSink;

impl Sink for StdoutSink {
    fn emit(&mut self, ev: &Event) {
        println!("{}", ev.to_line());
        let _ = std::io::stdout().flush();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = PathBuf::from(
        std::env::var(DATA_DIR_ENV).unwrap_or_else(|_| DEFAULT_DATA_DIR.to_string()),
    );
    let store = Store::new(&data_dir);
    let _ = store.ensure_dirs();

    let code = match args.first().map(String::as_str) {
        Some("status") => cmd_status(&store),
        Some("credits") => cmd_credits(&store),
        Some("credit-diffs") => cmd_credit_diffs(&store),
        Some("accounts") => cmd_accounts(&store, args.get(1).map(String::as_str)),
        Some("add") => cmd_add(&store, &args[1..]),
        Some("remove") => cmd_remove(&store, &args[1..]),
        Some("enable") => cmd_set_enabled(&store, &args[1..], true),
        Some("disable") => cmd_set_enabled(&store, &args[1..], false),
        Some("run-now") => cmd_run_now(&store, args.get(1).map(String::as_str)),
        Some("config") => cmd_config(&store, &args[1..]),
        Some("log") => cmd_log(&store, args.get(1)),
        Some("checkin-log") => cmd_checkin_log(&store),
        Some("trae-login-url") => cmd_trae_login_url(&store),
        Some("trae-wait") => cmd_trae_wait(args.get(1).map(|s| s.as_str())),
        Some("trae-callback") => cmd_trae_callback(&store, args.get(1).map(|s| s.as_str())),
        Some("trae-refresh") => cmd_trae_refresh(&store, args.get(1).map(String::as_str)),
        Some("wb-start") => cmd_wb_start(&store),
        Some("wb-poll") => cmd_wb_poll(&store, args.get(1).map(String::as_str)),
        Some("qoder-start") => cmd_qoder_start(args.get(1).and_then(|s| Some(s.as_str()))),
        Some("qoder-wait") => cmd_qoder_wait(args.get(1).map(|s| s.as_str())),
        Some("daemon-pid") => cmd_daemon_pid(&store),
        _ => {
            print_usage();
            2
        }
    };
    std::process::exit(code);
}

fn print_usage() {
    eprintln!(
        "usage: traeclockctl <status|accounts|add|remove|enable|disable|run-now|config|log|checkin-log|\
trae-login-url|trae-callback|trae-refresh|wb-start|wb-poll|qoder-start|qoder-wait|credits|credit-diffs|daemon-pid> [args]"
    );
}

fn ok_json(v: serde_json::Value) -> i32 {
    println!("{}", v);
    0
}

fn err_json(msg: String) -> i32 {
    println!("{}", serde_json::json!({ "ok": false, "error": msg }));
    1
}

fn cmd_credits(store: &Store) -> i32 {
    let cfg = store.config();
    ok_json(tc_core::credits::refresh_all(store, &cfg))
}

/// Check-in credit changes (before/after per round), newest first.
fn cmd_credit_diffs(store: &Store) -> i32 {
    let state = store.state();
    let mut diffs = state.credit_diffs;
    diffs.sort_by(|a, b| (b.date.clone() + " " + &b.time).cmp(&(a.date.clone() + " " + &a.time)));
    ok_json(serde_json::json!({ "ok": true, "diffs": diffs }))
}

fn cmd_status(store: &Store) -> i32 {
    let cfg = store.config();
    let acc = store.accounts();
    let state = store.state();
    let now = chrono::Local::now();
    let mut next: Option<serde_json::Value> = None;
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
        let ran_today =
            last_date.starts_with(&now.format("%Y-%m-%d").to_string());
        let today_str = now.format("%Y-%m-%d").to_string();
        let s = tc_core::schedule::seconds_until_next(
            &cfg,
            app,
            if ran_today { Some(today_str.as_str()) } else { None },
            now,
        );
        let entry = serde_json::json!({ "app": app, "in_secs": s, "ran_today": ran_today });
        if next.as_ref().map(|n| n["in_secs"].as_i64().unwrap_or(i64::MAX) > s).unwrap_or(true) {
            next = Some(entry);
        }
    }
    ok_json(serde_json::json!({
        "ok": true,
        "data_dir": store.data_dir.display().to_string(),
        "daemon_pid": std::fs::read_to_string(store.data_dir.join("daemon.pid")).ok()
            .and_then(|s| s.trim().parse::<u32>().ok()),
        "config": cfg,
        "counts": count_accounts(&acc),
        "last_rounds": state.last_rounds,
        "next": next,
    }))
}

fn count_accounts(acc: &AccountsFile) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for app in [APP_TRAE, APP_WB, APP_QODER] {
        let list: Vec<&Account> = acc.accounts.iter().filter(|a| a.app == app).collect();
        m.insert(
            app.to_string(),
            serde_json::json!({
                "total": list.len(),
                "enabled": list.iter().filter(|a| a.enabled).count(),
                "needs_relogin": list.iter().filter(|a| a.needs_relogin).count(),
            }),
        );
    }
    serde_json::Value::Object(m)
}

fn mask(s: &Option<String>) -> String {
    match s {
        Some(v) if v.len() > 12 => format!("{}…{}", &v[..6], &v[v.len() - 4..]),
        Some(v) if !v.is_empty() => "…".to_string(),
        _ => String::new(),
    }
}

fn cmd_accounts(store: &Store, app: Option<&str>) -> i32 {
    let acc = store.accounts();
    let cfg = store.config();
    let mut out = Vec::new();
    for a in &acc.accounts {
        if let Some(filter) = app {
            if a.app != filter {
                continue;
            }
        }
        let jwt_info = a.jwt.as_deref().map(tc_core::jwt::parse);
        out.push(serde_json::json!({
            "app": a.app,
            "user_id": a.user_id,
            "name": a.name,
            "group": a.group,
            "enabled": a.enabled,
            "needs_relogin": a.needs_relogin,
            "relogin_reason": a.relogin_reason,
            "jwt_status": tc_core::jwt::status_of(jwt_info.as_ref().and_then(|i| i.exp_hours)),
            "jwt_exp_hours": jwt_info.as_ref().and_then(|i| i.exp_hours),
            "jwt_masked": mask(&a.jwt),
            "has_refresh_token": a.refresh_token.as_ref().map(|s| !s.is_empty()).unwrap_or(false),
            "access_token_expires_at": a.access_token_expires_at,
            "device_id": cfg.device_map.get(&a.user_id).map(|d| d.device_id.clone())
                .or_else(|| tc_core::fingerprint::derive_device(&a.user_id).device_id.into()),
        }));
    }
    ok_json(serde_json::json!({ "ok": true, "accounts": out }))
}

fn cmd_add(store: &Store, args: &[String]) -> i32 {
    let Some(app) = args.first() else { return err_json("缺少 app 参数".into()) };
    let raw = args.get(1..).map(|r| r.join(" ")).unwrap_or_default();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return err_json("账号 JSON 解析失败".into());
    };
    let user_id = tc_core::envelope::dig(&v, &["user_id", "userId", "uid"])
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if user_id.is_empty() {
        return err_json("账号 JSON 缺少 user_id/uid".into());
    }
    let mut file = store.accounts();
    let now = tc_core::util::now_iso_local();
    if let Some(existing) = file
        .accounts
        .iter_mut()
        .find(|a| a.app == *app && a.user_id == user_id)
    {
        merge_account(existing, &v, app, &now);
    } else {
        let mut a = Account {
            app: app.to_string(),
            user_id: user_id.clone(),
            name: tc_core::envelope::dig(&v, &["name", "nickname"])
                .and_then(|x| x.as_str())
                .unwrap_or(&user_id)
                .to_string(),
            enabled: true,
            created_at: Some(now.clone()),
            ..Default::default()
        };
        merge_account(&mut a, &v, app, &now);
        file.accounts.push(a);
    }
    match store.save_accounts(&file) {
        Ok(_) => ok_json(serde_json::json!({ "ok": true, "user_id": user_id })),
        Err(e) => err_json(e),
    }
}

fn merge_account(a: &mut Account, v: &serde_json::Value, app: &str, now: &str) {
    let dig = |keys: &[&str]| {
        tc_core::envelope::dig(v, keys).and_then(|x| x.as_str()).map(String::from)
    };
    if let Some(n) = dig(&["name", "nickname"]) {
        if !n.is_empty() {
            a.name = n;
        }
    }
    if let Some(g) = dig(&["group"]) {
        a.group = Some(g);
    }
    a.updated_at = Some(now.to_string());
    match app {
        APP_TRAE => {
            if let Some(j) = dig(&["jwt", "access_token", "accessToken"]) {
                a.jwt = Some(j);
            }
            if let Some(rt) = dig(&["refresh_token", "refreshToken"]) {
                a.refresh_token = Some(rt);
            }
            if let Some(exp) = tc_core::envelope::dig(v, &["refresh_token_expires_at"]).and_then(|x| x.as_i64()) {
                a.refresh_token_expires_at = Some(if exp > 10_000_000_000 { exp / 1000 } else { exp });
            }
        }
        APP_WB => {
            if let Some(at) = dig(&["access_token", "accessToken", "token"]) {
                a.access_token = Some(at);
            }
            if let Some(rt) = dig(&["refresh_token", "refreshToken"]) {
                a.refresh_token = Some(rt);
            }
            if let Some(d) = dig(&["domain"]) {
                a.domain = Some(d);
            }
            a.needs_relogin = false;
            a.relogin_reason.clear();
        }
        APP_QODER => {
            // whole creds blob stored for Qoder
            a.qoder_creds = Some(v.clone());
        }
        _ => {}
    }
}

fn cmd_remove(store: &Store, args: &[String]) -> i32 {
    let (Some(app), Some(uid)) = (args.first(), args.get(1)) else {
        return err_json("用法: remove <app> <user_id>".into());
    };
    let mut file = store.accounts();
    let before = file.accounts.len();
    file.accounts.retain(|a| !(a.app == *app && a.user_id == *uid));
    if file.accounts.len() == before {
        return err_json("账号不存在".into());
    }
    match store.save_accounts(&file) {
        Ok(_) => ok_json(serde_json::json!({ "ok": true })),
        Err(e) => err_json(e),
    }
}

fn cmd_set_enabled(store: &Store, args: &[String], enabled: bool) -> i32 {
    let (Some(app), Some(uid)) = (args.first(), args.get(1)) else {
        return err_json("用法: enable|disable <app> <user_id>".into());
    };
    let mut file = store.accounts();
    let mut hit = false;
    for a in file.accounts.iter_mut() {
        if a.app == *app && a.user_id == *uid {
            a.enabled = enabled;
            a.updated_at = Some(tc_core::util::now_iso_local());
            hit = true;
        }
    }
    if !hit {
        return err_json("账号不存在".into());
    }
    match store.save_accounts(&file) {
        Ok(_) => ok_json(serde_json::json!({ "ok": true })),
        Err(e) => err_json(e),
    }
}

fn cmd_run_now(store: &Store, app: Option<&str>) -> i32 {
    let cfg = store.config();
    let mut sink = StdoutSink;
    let v = match app {
        Some(a) => match tc_core::runner::run_one(store, &cfg, a, &mut sink) {
            Some(v) => v,
            None => return err_json(format!("未知或未启用的 app: {a}")),
        },
        None => tc_core::runner::run_all(store, &cfg, &mut sink),
    };
    ok_json(serde_json::json!({ "ok": true, "result": v }))
}

fn cmd_config(store: &Store, args: &[String]) -> i32 {
    if args.is_empty() {
        return ok_json(serde_json::json!({ "ok": true, "config": store.config() }));
    }
    let raw = args.join(" ");
    let Ok(patch) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return err_json("config JSON 解析失败".into());
    };
    let cfg = store.config();
    let merged: AppConfig = match serde_json::from_value::<AppConfig>(merge_json(
        serde_json::to_value(&cfg).unwrap_or_default(),
        patch,
    )) {
        Ok(c) => c.sanitized(),
        Err(_) => cfg.clone(),
    };
    match store.save_config(&merged) {
        Ok(_) => ok_json(serde_json::json!({ "ok": true, "config": merged })),
        Err(e) => err_json(e),
    }
}

/// Shallow-deep merge for config patches (objects merge, arrays replace).
fn merge_json(base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    match (base, patch) {
        (serde_json::Value::Object(mut b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                let nv = if v.is_object() {
                    merge_json(b.get(&k).cloned().unwrap_or(serde_json::Value::Null), v)
                } else {
                    v
                };
                b.insert(k, nv);
            }
            serde_json::Value::Object(b)
        }
        (_, p) => p,
    }
}

fn cmd_log(store: &Store, n: Option<&String>) -> i32 {
    let n: usize = n.and_then(|s| s.parse().ok()).unwrap_or(200);
    ok_json(serde_json::json!({ "ok": true, "lines": store.tail_events(n) }))
}

fn cmd_checkin_log(store: &Store) -> i32 {
    let path = store.data_dir.join("logs").join("checkin.log");
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = raw.lines().collect();
    let start = lines.len().saturating_sub(200);
    ok_json(serde_json::json!({ "ok": true, "lines": lines[start..] }))
}

// ── Trae OAuth (login_url + loopback callback receiver) ─────────────────────

const TRAE_LOOPBACK_PORT: u16 = tc_core::trae_oauth::LOOPBACK_PORT;

fn cmd_trae_login_url(store: &Store) -> i32 {
    let identity = load_identity(store);
    // KEY FIX: persist the identity NOW — the authorization page registers
    // this device; if load_identity regenerated a different key on the
    // exchange call, the server rejects with 20403 Device not match.
    if let Err(e) = store.save_json("trae_identity.json", &identity) {
        return err_json(e);
    }
    let (url, pending) = match tc_core::trae_oauth::login_url(&identity, "Android", "15") {
        Ok(x) => x,
        Err(e) => return err_json(e),
    };
    if let Err(e) = store.save_json("trae_pending_login.json", &pending) {
        return err_json(e);
    }
    ok_json(serde_json::json!({ "ok": true, "url": url, "state": pending.state, "port": TRAE_LOOPBACK_PORT }))
}

fn cmd_trae_callback(store: &Store, url: Option<&str>) -> i32 {
    let Some(url) = url else { return err_json("缺少回调 URL 参数".into()) };
    finish_trae_login(store, url)
}

fn cmd_trae_refresh(store: &Store, uid: Option<&str>) -> i32 {
    let Some(uid) = uid else { return err_json("缺少 user_id 参数".into()) };
    let file = store.accounts();
    let Some(acc) = file.accounts.iter().find(|a| a.app == APP_TRAE && a.user_id == uid) else {
        return err_json("账号不存在".into());
    };
    let Some(rt) = acc.refresh_token.as_ref().filter(|s| !s.is_empty()) else {
        return err_json("该账号无 refresh_token".into());
    };
    let identity = load_identity(store);
    let agent = tc_core::http::agent(30);
    match tc_core::trae_oauth::exchange_token_refresh(&agent, &identity, rt) {
        Ok(r) => {
            let jwt_full = format!("Cloud-IDE-JWT {}", r.access_token);
            let info = tc_core::jwt::parse(&jwt_full);
            if let Some(new_uid) = &info.user_id {
                if new_uid != uid {
                    return err_json(format!("刷新后 user_id 不匹配: 期望={uid}, 实际={new_uid}"));
                }
            }
            let mut file = file.clone();
            for a in file.accounts.iter_mut() {
                if a.app == APP_TRAE && a.user_id == uid {
                    a.jwt = Some(jwt_full.clone());
                    if let Some(ref rt2) = r.refresh_token {
                        a.refresh_token = Some(rt2.clone());
                    }
                    a.refresh_token_fails = 0;
                    a.refresh_token_invalid = false;
                    a.updated_at = Some(tc_core::util::now_iso_local());
                }
            }
            let _ = store.save_accounts(&file);
            let _ = store.save_json("trae_identity.json", &identity);
            ok_json(serde_json::json!({ "ok": true, "exp_hours": info.exp_hours }))
        }
        Err(e) => {
            // record failure (server-refused semantics simplified: count only)
            let mut file = file.clone();
            for a in file.accounts.iter_mut() {
                if a.app == APP_TRAE && a.user_id == uid {
                    a.refresh_token_fails += 1;
                }
            }
            let _ = store.save_accounts(&file);
            err_json(e)
        }
    }
}

fn load_identity(store: &Store) -> tc_core::trae_oauth::ModuleIdentity {
    let saved: Option<tc_core::trae_oauth::ModuleIdentity> = store.read_json_opt("trae_identity.json");
    tc_core::trae_oauth::ModuleIdentity::load_or_create(
        saved.as_ref().map(|s| s.private_key_pem.as_str()),
    )
    .unwrap_or_default()
}

// ── WorkBuddy OAuth (state → browser → poll) ────────────────────────────────

const WB_OAUTH_BASE: &str = "https://copilot.tencent.com";
const WB_OAUTH_ORIGIN: &str = "https://www.codebuddy.cn";
const WB_OAUTH_UA: &str = "CLI/2.63.2 CodeBuddy/2.63.2";

fn wb_headers() -> Vec<(String, String)> {
    vec![
        ("User-Agent".to_string(), WB_OAUTH_UA.to_string()),
        ("Accept".to_string(), "application/json, text/plain, */*".to_string()),
        ("X-Requested-With".to_string(), "XMLHttpRequest".to_string()),
        ("Origin".to_string(), WB_OAUTH_ORIGIN.to_string()),
        ("Referer".to_string(), format!("{WB_OAUTH_ORIGIN}/")),
        ("Content-Type".to_string(), "application/json".to_string()),
    ]
}

fn cmd_wb_start(store: &Store) -> i32 {
    let agent = tc_core::http::agent(15);
    let (status, body, raw) = tc_core::http::post_json(
        &agent,
        &format!("{WB_OAUTH_BASE}/v2/plugin/auth/state?platform=CLI"),
        &wb_headers(),
        &serde_json::json!({}),
    );
    if status != 200 {
        return err_json(format!("auth/state 失败（HTTP {status}）: {}", trunc(&raw, 120)));
    }
    let b = body.ok_or(()).unwrap_or(serde_json::Value::Null);
    let Some(state_id) = tc_core::envelope::dig(&b, &["state", "authState", "auth_state"])
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return err_json("auth/state 响应中未找到 state".into());
    };
    // authUrl from server only when it already carries the state param
    let auth_url = tc_core::envelope::dig(
        &b,
        &["auth_url", "authUrl", "login_url", "loginUrl", "redirect_url", "url"],
    )
    .and_then(|v| v.as_str())
    .filter(|u| u.starts_with("https://") && u.contains("state="))
    .map(String::from)
    .unwrap_or_else(|| format!("{WB_OAUTH_BASE}/login?platform=CLI&state={state_id}"));
    let pending = serde_json::json!({ "state": state_id, "started_at": tc_core::util::now_iso_local() });
    if let Err(e) = store.save_json("wb_pending_login.json", &pending) {
        return err_json(e);
    }
    ok_json(serde_json::json!({ "ok": true, "state": state_id, "auth_url": auth_url }))
}

fn cmd_wb_poll(store: &Store, state_id: Option<&str>) -> i32 {
    let Some(state_id) = state_id else { return err_json("缺少 state 参数".into()) };
    let agent = tc_core::http::agent(15);
    let (status, body, _raw) = tc_core::http::get_json(
        &agent,
        &format!("{WB_OAUTH_BASE}/v2/plugin/auth/token?state={state_id}"),
        &wb_headers(),
    );
    if status != 200 {
        return ok_json(serde_json::json!({ "ok": true, "pending": true, "http": status }));
    }
    let b = body.unwrap_or(serde_json::Value::Null);
    let token = tc_core::envelope::dig(&b, &["accessToken"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return ok_json(serde_json::json!({ "ok": true, "pending": true }));
    }
    let refresh = tc_core::envelope::dig(&b, &["refreshToken"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_in = tc_core::envelope::dig(&b, &["expiresIn"]).and_then(|v| v.as_i64());
    // ④ account profile
    let auth_h = {
        let mut h = wb_headers();
        h.push(("Authorization".to_string(), format!("Bearer {token}")));
        h
    };
    let (_s2, acct_body, _r) = tc_core::http::get_json(
        &agent,
        &format!("{WB_OAUTH_BASE}/v2/plugin/login/account?state={state_id}"),
        &auth_h,
    );
    let acct = acct_body.unwrap_or(serde_json::Value::Null);
    let mut uid = tc_core::envelope::dig(&acct, &["uid", "userId", "user_id"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let nickname = tc_core::envelope::dig(&acct, &["nickname", "nickName", "name"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if uid.is_empty() {
        uid = tc_core::jwt::claims(&token)
            .and_then(|c| c.get("sub").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_default();
    }
    if uid.is_empty() {
        uid = format!("wb-{}", tc_core::util::sha256_hex(&token)[..12].to_string());
    }
    let exp_s = expires_in
        .map(|s| chrono::Utc::now().timestamp() + s)
        .or_else(|| tc_core::jwt::claims(&token).and_then(|c| c.get("exp").and_then(|v| v.as_i64())));
    // upsert account
    let mut file = store.accounts();
    let now = tc_core::util::now_iso_local();
    if let Some(existing) = file
        .accounts
        .iter_mut()
        .find(|a| a.app == APP_WB && (a.user_id == uid || a.user_id.is_empty()))
    {
        existing.user_id = uid.clone();
        if !nickname.is_empty() {
            existing.name = nickname.clone();
        }
        existing.access_token = Some(token);
        existing.refresh_token = Some(refresh);
        existing.access_token_expires_at = exp_s;
        existing.needs_relogin = false;
        existing.relogin_reason.clear();
        existing.updated_at = Some(now);
    } else {
        file.accounts.push(Account {
            app: APP_WB.into(),
            user_id: uid.clone(),
            name: if nickname.is_empty() { uid.clone() } else { nickname.clone() },
            enabled: true,
            access_token: Some(token),
            refresh_token: Some(refresh),
            access_token_expires_at: exp_s,
            created_at: Some(now),
            ..Default::default()
        });
    }
    if let Err(e) = store.save_accounts(&file) {
        return err_json(e);
    }
    ok_json(serde_json::json!({ "ok": true, "pending": false, "user_id": uid, "name": nickname }))
}

// ── Qoder device flow (byte-faithful: local PKCE session, no start endpoint) ─

fn cmd_qoder_start(compat: Option<&str>) -> i32 {
    // The device flow session is generated LOCALLY (nonce/verifier/machine_id),
    // exactly like the source: there is no server-side start endpoint.
    let flow = match compat {
        Some("compat") => tc_core::qoder::DeviceFlow::new_compat(),
        _ => tc_core::qoder::DeviceFlow::new(),
    };
    // persist the session so a separate qoder-poll/qoder-wait call can resume it
    if let Err(e) = store_at_data_dir().save_json("qoder_pending_flow.json", &flow) {
        return err_json(e);
    }
    ok_json(serde_json::json!({
        "ok": true, "auth_url": flow.auth_url, "nonce": flow.nonce,
    }))
}

fn cmd_qoder_wait(timeout: Option<&str>) -> i32 {
    let timeout_s: u64 = timeout.and_then(|s| s.parse().ok()).unwrap_or(180);
    let store = store_at_data_dir();
    // resume a persisted session (from qoder-start) or create one now
    let mut flow: tc_core::qoder::DeviceFlow = store
        .read_json_opt("qoder_pending_flow.json")
        .unwrap_or_else(tc_core::qoder::DeviceFlow::new);
    // automatic compat fallback: if a previous normal-mode attempt hit the
    // authorization page's 「参数无效」 (caller passes "compat"), switch mode
    if std::env::args().any(|a| a == "compat") {
        flow = tc_core::qoder::DeviceFlow::new_compat();
    }
    println!(
        "{}",
        serde_json::json!({ "ok": true, "stage": "started", "auth_url": flow.auth_url })
    );
    let _ = std::io::stdout().flush();
    let agent = tc_core::http::agent(15);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
    loop {
        if std::time::Instant::now() > deadline {
            return err_json("授权超时".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(tc_core::qoder::POLL_INTERVAL_MS));
        let (status, body) = tc_core::qoder::device_flow_poll_once(&agent, &flow);
        match status {
            // pending: HTTP 404 {"errorCode":"NotFound"} (R-10 capture)
            404 => continue,
            200 => {
                let Some(b) = body else { return err_json("授权响应非 JSON".into()) };
                let Some((creds, uid)) = tc_core::qoder::parse_poll_success(&b, &flow.nonce) else {
                    return err_json("授权响应校验失败（nonce 不匹配或缺少令牌字段）".into());
                };
                return qoder_store_creds(&store, creds, uid);
            }
            400 | 401 | 403 | 410 => {
                let msg = body
                    .as_ref()
                    .and_then(|b| tc_core::envelope::dig(b, &["errorMessage", "error_message", "message"]).and_then(|v| v.as_str()).map(String::from))
                    .unwrap_or_else(|| format!("授权会话失效（HTTP {status}）"));
                return err_json(msg);
            }
            _ => continue,
        }
    }
}

fn qoder_store_creds(store: &Store, creds: tc_core::qoder::QoderCreds, uid: String) -> i32 {
    let mut file = store.accounts();
    let now = tc_core::util::now_iso_local();
    let profile = tc_core::qoder::DeviceProfile::generate(&uid);
    let mut creds = creds;
    if creds.machine_id.is_empty() {
        creds.machine_id = profile.machine_code.clone();
    }
    if let Some(existing) = file
        .accounts
        .iter_mut()
        .find(|a| a.app == APP_QODER && a.user_id == uid)
    {
        existing.qoder_creds = Some(creds.to_value());
        existing.qoder_device_profile = Some(serde_json::to_value(&profile).unwrap_or_default());
        existing.needs_relogin = false;
        existing.updated_at = Some(now);
    } else {
        file.accounts.push(Account {
            app: APP_QODER.into(),
            user_id: uid.clone(),
            name: if creds.nickname.is_empty() { uid.clone() } else { creds.nickname.clone() },
            enabled: true,
            qoder_creds: Some(creds.to_value()),
            qoder_device_profile: Some(serde_json::to_value(&profile).unwrap_or_default()),
            created_at: Some(now),
            ..Default::default()
        });
    }
    if let Err(e) = store.save_accounts(&file) {
        return err_json(e);
    }
    ok_json(serde_json::json!({ "ok": true, "user_id": uid }))
}

fn store_at_data_dir() -> Store {
    Store::new(&data_dir())
}

// ── Trae loopback callback receiver (the missing piece for mobile login) ─────

/// Listen on 127.0.0.1:17388 for the OAuth redirect, print a one-line status,
/// then finish the login exactly like `trae-callback <url>`.
/// The browser lands on `http://127.0.0.1:17388/authorize?refreshToken=…`;
/// we answer 302 to a local success page and complete the flow.
fn cmd_trae_wait(timeout: Option<&str>) -> i32 {
    use std::net::TcpListener;
    let timeout_s: u64 = timeout.and_then(|s| s.parse().ok()).unwrap_or(300);
    let listener = match TcpListener::bind(("127.0.0.1", tc_core::trae_oauth::LOOPBACK_PORT)) {
        Ok(l) => l,
        Err(e) => return err_json(format!("监听 127.0.0.1:{} 失败（端口被占用?）: {e}", tc_core::trae_oauth::LOOPBACK_PORT)),
    };
    println!(
        "{}",
        serde_json::json!({ "ok": true, "stage": "listening", "port": tc_core::trae_oauth::LOOPBACK_PORT })
    );
    let _ = std::io::stdout().flush();
    listener.set_nonblocking(true).ok();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
    loop {
        if std::time::Instant::now() > deadline {
            return err_json("等待回调超时：请确认在浏览器完成了登录并允许跳转 127.0.0.1".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                // read until end of headers (the request line carries the callback)
                let mut buf = [0u8; 8192];
                let mut req = Vec::new();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
                loop {
                    match std::io::Read::read(&mut stream, &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            req.extend_from_slice(&buf[..n]);
                            if req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() > 16384 {
                                break;
                            }
                        }
                    }
                }
                let req_text = String::from_utf8_lossy(&req);
                // request line: GET /authorize?refreshToken=... HTTP/1.1
                let target = req_text
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                let full_url = format!("http://127.0.0.1:{}{target}", tc_core::trae_oauth::LOOPBACK_PORT);
                // friendly browser page
                let body = "<html><body style='font-family:sans-serif;text-align:center;padding-top:80px'><h2>✅ 登录信息已接收</h2><p>可以关闭此页面，回到 TraeClock 查看结果。</p></body></html>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                drop(stream);
                // accept any callback carrying login payload (main path is
                // authCodeInfo=…, compat paths refreshToken=/code=)
                let has_params = full_url.contains("authCodeInfo=")
                    || full_url.contains("refreshToken=")
                    || full_url.contains("code=");
                if !has_params {
                    continue; // favicon or stray request; keep listening
                }
                let store = store_at_data_dir();
                return finish_trae_login(&store, &full_url);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => return err_json(format!("accept 失败: {e}")),
        }
    }
}

/// Shared finish path for trae-callback and trae-wait.
/// Main path (抓包固化): native_ide callback carries authCodeInfo →
/// ExchangeToken with PKCE verifier; compat: refreshToken / code / accessToken.
fn finish_trae_login(store: &Store, callback_url: &str) -> i32 {
    let cb = match tc_core::trae_oauth::parse_callback(callback_url) {
        Ok(c) => c,
        Err(e) => return err_json(e),
    };
    let pending: Option<tc_core::trae_oauth::PendingLogin> =
        store.read_json_opt("trae_pending_login.json");
    // CSRF（桌面 oauth.rs:622 语义）：回调带 loginTraceID 且有在途会话时必须一致；
    // authCode 回调缺 loginTraceID 且本机有在途会话 → 拒绝
    if let Some(p) = &pending {
        match &cb.login_trace_id {
            Some(tid) if *tid != p.state => {
                return err_json("loginTraceID 与会话 state 不匹配（CSRF 防护），已拒绝".into())
            }
            None if cb.auth_code.is_some() => {
                return err_json("OAuth 回调缺少 loginTraceID，无法确认与本机登录请求的对应关系，已拒绝".into())
            }
            _ => {}
        }
    }
    let identity = load_identity(store);
    let agent = tc_core::http::agent(30);
    let exchange = if let Some(code) = cb.auth_code.clone().or_else(|| {
        // 标准 code 参数也走 AuthCode 交换
        if cb.refresh_token.is_empty() && callback_url.contains("code=") {
            callback_url
                .split("code=")
                .nth(1)
                .map(|s| s.split('&').next().unwrap_or("").to_string())
        } else {
            None
        }
    }) {
        // 主路径：AuthCode + PKCE verifier（桌面 exchange_code 同款）
        let verifier = pending.map(|p| p.pkce_verifier).unwrap_or_default();
        tc_core::trae_oauth::exchange_token_auth_code(&agent, &identity, &code, &verifier, cb.host.as_deref())
    } else if !cb.refresh_token.is_empty() {
        tc_core::trae_oauth::exchange_token_refresh(&agent, &identity, &cb.refresh_token)
    } else {
        Err("回调缺少 authCodeInfo/refreshToken".into())
    };
    let result = match exchange {
        Ok(r) => r,
        Err(e) => return err_json(e),
    };
    let jwt_full = if result.access_token.starts_with("Cloud-IDE-JWT ") {
        result.access_token.clone()
    } else {
        format!("Cloud-IDE-JWT {}", result.access_token)
    };
    let info = tc_core::jwt::parse(&jwt_full);
    let uid = info
        .user_id
        .clone()
        .or_else(|| cb.user_id.clone())
        .unwrap_or_default();
    if uid.is_empty() {
        return err_json("无法从回调或 JWT 中获取 user_id".into());
    }
    // 换发 token 归属校验（refresh 交换场景）
    if let Some(cb_uid) = &cb.user_id {
        if !cb_uid.is_empty() && info.user_id.is_some() && &info.user_id.unwrap() != cb_uid {
            return err_json(format!("换发 token 归属不匹配: 期望={cb_uid}"));
        }
    }
    let _ = store.save_json("trae_identity.json", &identity);
    // issue #76 次因修复：新 JWT 绑定本次登录设备 → device_map 回写，
    // 签到/积分查询的 x-device-id 必须与 JWT 同源，否则严格指纹校验 401
    let mut cfg = store.config();
    let dev_entry = cfg
        .device_map
        .entry(uid.clone())
        .or_insert_with(|| tc_core::fingerprint::derive_device(&uid));
    let mut dev_entry = dev_entry.clone();
    dev_entry.device_id = identity.device_id.clone();
    cfg.device_map.insert(uid.clone(), dev_entry);
    if let Err(e) = store.save_config(&cfg) {
        eprintln!("device_map 回写失败（不阻断登录）: {e}");
    }
    // 昵称兜底：GetUserInfo（桌面 oauth.rs get_user_info 同款端点）
    let name = cb.user_name.clone().unwrap_or_else(|| {
        get_user_info(&agent, &jwt_full)
            .map(|(_, n)| if n.is_empty() { uid.clone() } else { n })
            .unwrap_or_else(|| {
                let head: String = uid.chars().take(8).collect();
                format!("账号_{head}")
            })
    });
    let mut file = store.accounts();
    let now = tc_core::util::now_iso_local();
    let merged = if let Some(existing) = file
        .accounts
        .iter_mut()
        .find(|a| a.app == APP_TRAE && a.user_id == uid)
    {
        existing.jwt = Some(jwt_full.clone());
        existing.refresh_token = result.refresh_token.clone().or(existing.refresh_token.clone());
        existing.updated_at = Some(now);
        true
    } else {
        file.accounts.push(Account {
            app: APP_TRAE.into(),
            user_id: uid.clone(),
            name: name.clone(),
            enabled: true,
            jwt: Some(jwt_full),
            refresh_token: result.refresh_token.clone(),
            created_at: Some(now),
            ..Default::default()
        });
        false
    };
    if let Err(e) = store.save_accounts(&file) {
        return err_json(e);
    }
    ok_json(serde_json::json!({
        "ok": true, "user_id": uid,
        "name": name, "merged": merged,
        "exp_hours": info.exp_hours,
    }))
}

/// GetUserInfo（桌面 oauth.rs:1313 同款端点与解析）
fn get_user_info(agent: &ureq::Agent, jwt_full: &str) -> Option<(String, String)> {
    let auth = if jwt_full.starts_with("Cloud-IDE-JWT ") {
        jwt_full.to_string()
    } else {
        format!("Cloud-IDE-JWT {}", jwt_full.trim())
    };
    let headers = vec![
        ("authorization".to_string(), auth),
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "*/*".to_string()),
    ];
    let (status, body, _) = tc_core::http::post_json(
        agent,
        "https://api.trae.com.cn/cloudide/api/v3/trae/GetUserInfo",
        &headers,
        &serde_json::json!({}),
    );
    if status != 200 {
        return None;
    }
    let b = body?;
    let data = b.get("data").or_else(|| b.get("result"))?;
    let user_id = tc_core::envelope::dig(data, &["user_id", "UserID", "userId"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let user_name = tc_core::envelope::dig(data, &["name", "user_name", "userName", "nickname"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((user_id, user_name))
}

fn cmd_daemon_pid(store: &Store) -> i32 {
    let pid = std::fs::read_to_string(store.data_dir.join("daemon.pid"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    ok_json(serde_json::json!({ "ok": true, "pid": pid }))
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var(DATA_DIR_ENV).unwrap_or_else(|_| DEFAULT_DATA_DIR.to_string()))
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
