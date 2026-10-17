//! Trae OAuth: authorization-URL construction, callback parsing, PKCE,
//! ExchangeToken with DeviceProof (ECDSA P-256 + SHA-256, P1363 r||s),
//! and lazy refresh policy — port of `commands/oauth.rs` (mobile paths:
//! the module owns its own EC key pair; the desktop storage.json extraction
//! path is intentionally not ported).

use base64::Engine;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::envelope::{dig, path};
use crate::http;
use crate::util::random_hex;

pub const OAUTH_CLIENT_ID: &str = "ono9krqynydwx5";
pub const OAUTH_EXCHANGE_URL: &str = "https://api.trae.com.cn/cloudide/api/v3/trae/oauth/ExchangeToken";
pub const AUTHORIZE_PAGE: &str = "https://www.trae.cn/authorization";
pub const OAUTH_PAGE_PLUGIN_VERSION: &str = "2.3.83560";
pub const OAUTH_PAGE_APP_VERSION: &str = "3.3.100";
/// Manager-app loopback redirect (same port as desktop for URL-format parity).
pub const LOOPBACK_PORT: u16 = 17388;
pub const REDIRECT_URI: &str = "http://127.0.0.1:17388/authorize";

// ── PKCE ─────────────────────────────────────────────────────────────────────

/// PKCE pair (S256): verifier 64 hex chars (RFC 7636 compliant, same as the
/// source's random_hex_result(64)), challenge = base64url(sha256(verifier)).
pub fn pkce_pair() -> Result<(String, String), String> {
    let verifier = random_hex(64);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok((verifier, challenge))
}

// ── module EC identity ───────────────────────────────────────────────────────

/// Module-owned device identity: EC P-256 key pair + stable device/machine ids.
/// The private key persists in the store; device ids derive from the public key
/// so they survive key re-import unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleIdentity {
    /// PKCS#8 PEM private key (persisted in the store, never logged).
    #[serde(default)]
    pub private_key_pem: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub machine_id: String,
}

impl ModuleIdentity {
    /// Load from PEM, or generate a fresh identity on first use.
    pub fn load_or_create(pem: Option<&str>) -> Result<ModuleIdentity, String> {
        if let Some(pem) = pem.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let signing = SigningKey::from_pkcs8_pem(pem)
                .map_err(|e| format!("模块身份私钥解析失败: {e}"))?;
            return Ok(Self::from_signing_key(pem.to_string(), &signing));
        }
        let mut rng = rand::rngs::OsRng;
        let signing = SigningKey::random(&mut rng);
        let pem = signing
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| format!("私钥导出失败: {e}"))?
            .to_string();
        Ok(Self::from_signing_key(pem, &signing))
    }

    fn from_signing_key(pem: String, signing: &SigningKey) -> ModuleIdentity {
        let pub_der = signing
            .verifying_key()
            .to_public_key_der()
            .expect("p256 public key DER encoding is infallible");
        let pub_hex = crate::util::to_hex_lower(pub_der.as_bytes());
        ModuleIdentity {
            private_key_pem: pem,
            device_id: pub_hex[0..15].chars().map(|c| char::from(b'0' + (c as u8 % 10))).collect(),
            machine_id: pub_hex[15..47]
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { 'A' })
                .collect(),
        }
    }
}

// ── authorization URL ────────────────────────────────────────────────────────

/// Pending login session (CSRF + PKCE + chosen device), stored by the caller
/// between URL construction and callback verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingLogin {
    pub state: String,
    pub pkce_verifier: String,
    pub device_id: String,
}

/// Build the authorization page URL (port of `oauth_get_login_url`).
/// `hostname` is the device brand shown to the authorization page.
pub fn login_url(
    identity: &ModuleIdentity,
    hostname: &str,
    os_version: &str,
) -> Result<(String, PendingLogin), String> {
    let trace_id = random_hex(32);
    let (pkce_verifier, code_challenge) = pkce_pair()?;
    let url = format!(
        "{AUTHORIZE_PAGE}?\
        login_version=1\
        &auth_from=trae\
        &login_channel=native_ide\
        &plugin_version={plugin_version}\
        &auth_type=local\
        &client_id={client_id}\
        &redirect=0\
        &login_trace_id={trace_id}\
        &auth_callback_url={redirect_uri}\
        &machine_id={machine_id}\
        &device_id={device_id}\
        &x_device_id={device_id}\
        &x_machine_id={machine_id}\
        &x_device_brand={hostname}\
        &x_device_type=android\
        &x_os_version={os_version}\
        &x_env=\
        &x_app_version={app_version}\
        &x_app_type=stable\
        &code_challenge={code_challenge}\
        &code_challenge_method=S256\
        &channel_name=common",
        plugin_version = OAUTH_PAGE_PLUGIN_VERSION,
        client_id = OAUTH_CLIENT_ID,
        redirect_uri = urlencoding_encode(REDIRECT_URI),
        machine_id = identity.machine_id,
        device_id = identity.device_id,
        hostname = urlencoding_encode(hostname),
        os_version = urlencoding_encode(os_version),
        app_version = OAUTH_PAGE_APP_VERSION,
        code_challenge = code_challenge,
    );
    Ok((
        url,
        PendingLogin {
            state: trace_id,
            pkce_verifier,
            device_id: identity.device_id.clone(),
        },
    ))
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ── callback parsing ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallbackInfo {
    pub refresh_token: String,
    pub access_token: Option<String>,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub login_trace_id: Option<String>,
    /// 授权页回传的 API 域（main.js 逆向：交换 URL = ${host}/trae/api/v3/oauth/ExchangeToken）。
    /// native_ide 主路径回调必带；缺失时调用方回退默认域。
    pub host: Option<String>,
    /// 主路径凭证：authCodeInfo JSON 里的 AuthCode（抓包固化 2026-09-16：
    /// native_ide 流程 302 回调参数为 authCodeInfo 而非 refreshToken/code）。
    pub auth_code: Option<String>,
}

/// Parse the loopback callback URL. Three accepted shapes (desktop parity):
/// ① 主路径 `?authCodeInfo=<urlenc JSON>{"AuthCode":…}&userInfo=<urlenc JSON>&loginTraceID=…&host=…`
/// ② 兼容 `?refreshToken=…&accessToken=…&userId=…`
/// ③ 标准 `?code=…`（需 ExchangeToken + PKCE verifier）。
pub fn parse_callback(callback_url: &str) -> Result<CallbackInfo, String> {
    let query = callback_url.split('?').nth(1).ok_or("回调 URL 中缺少查询参数")?;
    let mut params = std::collections::HashMap::new();
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let k = kv.next().unwrap_or("").to_string();
        let v = kv.next().unwrap_or("").to_string();
        params.insert(k, url_decode(&v));
    }

    // 主路径：authCodeInfo JSON 展开（desktop oauth.rs:583-616 同款）
    let mut auth_code: Option<String> = None;
    if let Some(raw) = params.get("authCodeInfo").filter(|s| !s.is_empty()) {
        let v: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| format!("authCodeInfo 解析失败（{e}）：授权页回调格式异常"))?;
        let code = v
            .get("AuthCode")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("authCodeInfo 中缺少 AuthCode 字段")?
            .to_string();
        auth_code = Some(code);
    }
    // userInfo JSON 补全身份（UserID/ScreenName/NickName/AvatarUrl）
    let mut user_id = params
        .get("userId")
        .or_else(|| params.get("user_id"))
        .or_else(|| params.get("UserID"))
        .cloned();
    let mut user_name = params.get("userName").or_else(|| params.get("username")).cloned();
    if let Some(raw) = params.get("userInfo") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            if user_id.is_none() {
                user_id = v.get("UserID").and_then(|x| x.as_str()).map(String::from);
            }
            if user_name.is_none() {
                user_name = v
                    .get("ScreenName")
                    .or_else(|| v.get("NickName"))
                    .and_then(|x| x.as_str())
                    .map(String::from);
            }
        }
    }

    let rt = params
        .get("refreshToken")
        .or_else(|| params.get("refresh_token"))
        .cloned()
        .unwrap_or_default();
    let code = params.get("code").cloned().unwrap_or_default();
    if rt.is_empty() && code.is_empty() && auth_code.is_none() {
        return Err("回调 URL 缺少 authCodeInfo/refreshToken/code 参数".into());
    }
    Ok(CallbackInfo {
        refresh_token: rt,
        access_token: params.get("accessToken").or_else(|| params.get("access_token")).cloned(),
        user_id,
        user_name,
        login_trace_id: params.get("loginTraceID").or_else(|| params.get("login_trace_id")).cloned(),
        host: params.get("host").filter(|s| !s.is_empty()).cloned(),
        auth_code,
    })
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() + 1 && i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

// ── ExchangeToken ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Raw server body (for refresh_token_expires_at extraction by the caller).
    pub body: serde_json::Value,
}

/// DeviceProof JSON (PascalCase, Timestamp as JSON int):
/// signature over `POST\n<path>\n<ClientID>\n<RefreshToken>\n<Ts>\n<Nonce>`,
/// P1363 r||s 64B base64 (DER was rejected by the server, 20405).
pub fn device_proof(
    private_key_pem: &str,
    sign_path: &str,
    client_id: &str,
    auth_code: &str,
) -> Result<serde_json::Value, String> {
    let signing = SigningKey::from_pkcs8_pem(private_key_pem)
        .map_err(|e| format!("设备私钥解析失败: {e}"))?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let nonce = random_hex(32);
    let msg = format!("POST\n{sign_path}\n{client_id}\n{auth_code}\n{ts}\n{nonce}");
    let sig: Signature = signing.sign(msg.as_bytes());
    Ok(json!({
        "Signature": base64::engine::general_purpose::STANDARD.encode(sig.to_bytes()),
        "Timestamp": ts,
        "Nonce": nonce,
    }))
}

/// Refresh-token exchange (port of `exchange_token_refresh`):
/// `POST {host}/trae/api/v3/oauth/ExchangeToken` with DeviceProof.
pub fn exchange_token_refresh(
    agent: &ureq::Agent,
    identity: &ModuleIdentity,
    refresh_token: &str,
) -> Result<ExchangeResult, String> {
    let sign_path = "/trae/api/v3/oauth/ExchangeToken";
    let proof = device_proof(&identity.private_key_pem, sign_path, OAUTH_CLIENT_ID, refresh_token)?;
    let url = format!("https://api.trae.com.cn{sign_path}");
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-device-id".to_string(), identity.device_id.clone()),
    ];
    let payload = json!({
        "ClientID": OAUTH_CLIENT_ID,
        "RefreshToken": refresh_token,
        "DeviceProof": proof,
    });
    let (status, body, raw) = http::post_json(agent, &url, &headers, &payload);
    if status == 0 {
        return Err("ExchangeToken 网络不可达".into());
    }
    let b = body.unwrap_or(json!({}));
    // server-refusal detection: numeric legacy code != 0, volcano envelope 20101
    let legacy_code = b.get("code").and_then(|v| v.as_i64());
    let env_code = path(&b, &["ResponseMetadata", "Error", "Code"])
        .and_then(|v| v.as_str())
        .map(String::from);
    if legacy_code.map(|c| c != 0).unwrap_or(false)
        || env_code.as_deref() == Some("20101")
    {
        return Err(format!("ExchangeToken 被服务端拒绝（code={}）", legacy_code.unwrap_or(-1)));
    }
    let acc = dig(&b, &["accessToken", "access_token", "Access", "Token"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if acc.is_empty() {
        return Err(format!("ExchangeToken 响应缺少 accessToken（HTTP {status}）: {}", truncate(&raw, 160)));
    }
    Ok(ExchangeResult {
        access_token: acc,
        refresh_token: dig(&b, &["refreshToken", "refresh_token"])
            .and_then(|v| v.as_str())
            .map(String::from),
        body: b,
    },
    )
}

/// Token extraction inside an exchange response (desktop `find_token_value`
/// port): exact keys first, then case-insensitive keys, then whole-tree walk.
fn find_token_value(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match v {
        serde_json::Value::Object(m) => {
            for k in keys {
                if let Some(val) = m.get(*k).and_then(|x| x.as_str()) {
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
            }
            for (mk, val) in m {
                if keys.iter().any(|k| k.eq_ignore_ascii_case(mk)) {
                    if let Some(s) = val.as_str() {
                        if !s.is_empty() {
                            return Some(s.to_string());
                        }
                    }
                }
            }
            for val in m.values() {
                if let Some(found) = find_token_value(val, keys) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(a) => a.iter().find_map(|x| find_token_value(x, keys)),
        _ => None,
    }
}

/// Volcano envelope error (desktop `volcano_error` port):
/// `Error` / `ResponseMetadata.Error` → (Code, Message, StandardCode); None when absent.
fn volcano_error(body: &serde_json::Value) -> Option<(String, String, String)> {
    let err = body
        .get("Error")
        .or_else(|| path(body, &["ResponseMetadata", "Error"]))
        .or_else(|| dig(body, &["Error"]))?;
    let code = err
        .get("Code")
        .map(|v| {
            v.as_i64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(|s| s.trim().to_string()))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    if code.is_empty() {
        return None;
    }
    let field = |key: &str| err.get(key).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    Some((code, field("Message"), field("StandardCode")))
}

/// DeviceInfo payload for the AuthCode exchange (desktop main-variant shape:
/// DeviceInfo object + IDEVersion, no DeviceProof; DevicePublicKey included).
fn auth_code_payload(
    identity: &ModuleIdentity,
    auth_code: &str,
    code_verifier: &str,
) -> serde_json::Value {
    let pub_pem = device_public_key_pem(identity).unwrap_or_default();
    json!({
        "ClientID": OAUTH_CLIENT_ID,
        "AuthCode": auth_code,
        "CodeVerifier": code_verifier,
        "DeviceInfo": {
            "DeviceID": identity.device_id,
            "MachineID": identity.machine_id,
            "PlatformCode": "IDE_PC",
            "DeviceType": "PC",
            "DeviceName": "",
            "DeviceModel": "",
            "ClientVersion": OAUTH_PAGE_APP_VERSION,
            "DevicePublicKey": pub_pem,
            "DeviceBrand": "",
            "DeviceCPU": "",
            "OSInfo": "",
            "OSVersion": "",
        },
        "IDEVersion": OAUTH_PAGE_APP_VERSION,
    })
}

/// AuthCode exchange (callback 主路径：native_ide GetPCAuthCode → authCodeInfo).
/// Byte-parity headers with desktop `try_exchange_variant`: x-device-id +
/// x-app-id + x-platform-code + x-cloudide-token:"" (F-70: missing header → 20403,
/// non-empty → 20405). Variant chain: 回传 host 的 /trae/api/v3 → 固化域 → cloudide 旧域。
pub fn exchange_token_auth_code(
    agent: &ureq::Agent,
    identity: &ModuleIdentity,
    auth_code: &str,
    code_verifier: &str,
    host: Option<&str>,
) -> Result<ExchangeResult, String> {
    let payload = auth_code_payload(identity, auth_code, code_verifier);
    let new_path = "/trae/api/v3/oauth/ExchangeToken";
    let mut variants: Vec<(String, String)> = vec![(
        "ExchangeToken/DeviceInfo(host)".into(),
        format!("{}{new_path}", host.unwrap_or("https://api.trae.com.cn").trim_end_matches('/')),
    )];
    if host.is_none() || !host.unwrap_or("").contains("api.trae.com.cn") {
        variants.push((
            "ExchangeToken/DeviceInfo(fixed)".into(),
            format!("https://api.trae.com.cn{new_path}"),
        ));
        variants.push((
            "ExchangeToken/DeviceInfo(legacy)".into(),
            format!("https://api.trae.com.cn/cloudide/api/v3/trae/oauth/ExchangeToken"),
        ));
    }

    let mut errs: Vec<String> = Vec::new();
    for (tag, url) in &variants {
        let headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "*/*".to_string()),
            ("x-device-id".to_string(), identity.device_id.clone()),
            ("x-app-id".to_string(), "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8".to_string()),
            ("x-platform-code".to_string(), "IDE_PC".to_string()),
            // F-70 实测：x-cloudide-token 必须为空串；缺失 20403 / 非空 20405
            ("x-cloudide-token".to_string(), "".to_string()),
        ];
        let (status, body, raw) = http::post_json(agent, url, &headers, &payload);
        if status == 0 {
            errs.push(format!("{tag}: 网络不可达"));
            continue;
        }
        let b = body.unwrap_or(json!({}));
        // volcano envelope error → variant failure (keep probing other variants)
        if let Some((code, msg, std_code)) = volcano_error(&b) {
            if code != "0" {
                errs.push(format!("{tag}: code={code}/{}: {}", std_code, if msg.is_empty() { "未知错误" } else { &msg }));
                continue;
            }
        }
        // legacy numeric code
        if let Some(c) = b.get("code").and_then(|v| v.as_i64()) {
            if c != 0 {
                let msg = b.get("message").and_then(|v| v.as_str()).unwrap_or("未知错误");
                errs.push(format!("{tag}: code={c}: {msg}"));
                continue;
            }
        }
        // token extraction: Result/data/Data container → whole tree (desktop chain)
        const ACCESS_KEYS: &[&str] = &["AccessToken", "access_token", "token", "Jwt", "JWT"];
        const REFRESH_KEYS: &[&str] = &["RefreshToken", "refresh_token"];
        let container = b.get("Result").or_else(|| b.get("data")).or_else(|| b.get("Data"));
        let acc = container.and_then(|d| find_token_value(d, ACCESS_KEYS)).or_else(|| find_token_value(&b, ACCESS_KEYS));
        let refresh = container.and_then(|d| find_token_value(d, REFRESH_KEYS)).or_else(|| find_token_value(&b, REFRESH_KEYS));
        match acc {
            Some(a) if !a.is_empty() => {
                return Ok(ExchangeResult {
                    access_token: a,
                    refresh_token: refresh,
                    body: b,
                })
            }
            _ => {
                errs.push(format!("{tag}: 响应缺少 AccessToken（HTTP {status}）"));
            }
        }
        let _ = truncate(&raw, 80);
    }
    Err(format!("全部交换变体失败 → {}", errs.join(" | ")))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// SPKI PEM public key from the module identity (DeviceInfo.DevicePublicKey,
/// desktop `device_public_key_pem` shape).
pub fn device_public_key_pem(identity: &ModuleIdentity) -> Result<String, String> {
    use p256::pkcs8::DecodePrivateKey;
    use p256::pkcs8::EncodePublicKey;
    let signing = SigningKey::from_pkcs8_pem(&identity.private_key_pem)
        .map_err(|e| format!("设备私钥解析失败: {e}"))?;
    let pub_der = signing
        .verifying_key()
        .to_public_key_der()
        .map_err(|e| format!("公钥 SPKI 编码失败: {e}"))?;
    let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
    let b64 = base64::engine::general_purpose::STANDARD.encode(pub_der.as_bytes());
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str("-----END PUBLIC KEY-----");
    Ok(pem)
}

/// Lazy refresh gate (port of `lazy_refresh_needed`): JWT remaining > 48h AND
/// refresh_token not near expiry (> 48h) → skip (fewer rotations = smaller
/// double-client conflict surface).
pub fn lazy_refresh_needed(
    jwt_exp_timestamp: Option<i64>,
    refresh_token_expires_at: Option<i64>,
    now_ts: i64,
) -> bool {
    let jwt_fresh = match jwt_exp_timestamp {
        Some(exp) => (exp - now_ts) as f64 / 3600.0 > 48.0,
        None => false, // unknown expiry → refresh
    };
    let rt_expiring = refresh_token_expires_at
        .map(|exp| (exp - now_ts) as f64 / 3600.0 < 48.0)
        .unwrap_or(false); // no rt expiry info → do not trigger on rt alone
    !jwt_fresh || rt_expiring
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_pair_shape() {
        let (v, c) = pkce_pair().unwrap();
        assert_eq!(v.len(), 64);
        assert_eq!(c.len(), 43);
        assert!(!c.contains('='));
        let (v2, c2) = pkce_pair().unwrap();
        assert_ne!(v, v2);
        assert_ne!(c, c2);
    }

    #[test]
    fn identity_generation_is_loadable() {
        let id = ModuleIdentity::load_or_create(None).unwrap();
        assert!(!id.private_key_pem.is_empty());
        assert!(id.private_key_pem.contains("BEGIN PRIVATE KEY"));
        assert_eq!(id.device_id.len(), 15);
        assert!(id.device_id.chars().all(|c| c.is_ascii_digit()));
        // reload from the same PEM yields identical ids
        let id2 = ModuleIdentity::load_or_create(Some(&id.private_key_pem)).unwrap();
        assert_eq!(id.device_id, id2.device_id);
        assert_eq!(id.machine_id, id2.machine_id);
    }

    #[test]
    fn login_url_shape() {
        let id = ModuleIdentity::load_or_create(None).unwrap();
        let (url, pending) = login_url(&id, "Xiaomi", "15").unwrap();
        assert!(url.starts_with("https://www.trae.cn/authorization?"));
        assert!(url.contains("client_id=ono9krqynydwx5"));
        assert!(url.contains("login_channel=native_ide"));
        assert!(url.contains("x_device_type=android"));
        assert!(url.contains("x_device_brand=Xiaomi"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("auth_callback_url={}", urlencoding_encode(REDIRECT_URI))));
        assert_eq!(pending.state.len(), 32);
        assert_eq!(pending.pkce_verifier.len(), 64);
        assert_eq!(pending.device_id, id.device_id);
    }

    #[test]
    fn callback_parsing() {
        let cb = parse_callback(
            "http://127.0.0.1:17388/authorize?refreshToken=rt123&accessToken=at456&userId=u1&userName=%E5%B0%8F%E6%98%8E",
        )
        .unwrap();
        assert_eq!(cb.refresh_token, "rt123");
        assert_eq!(cb.access_token.as_deref(), Some("at456"));
        assert_eq!(cb.user_id.as_deref(), Some("u1"));
        assert_eq!(cb.user_name.as_deref(), Some("小明"));
        // code variant accepted (rt empty)
        let cb2 = parse_callback("http://127.0.0.1:17388/authorize?code=xyz&loginTraceID=t9").unwrap();
        assert_eq!(cb2.refresh_token, "");
        assert_eq!(cb2.login_trace_id.as_deref(), Some("t9"));
        // nothing usable → error
        assert!(parse_callback("http://127.0.0.1:17388/authorize?x=1").is_err());
        assert!(parse_callback("http://127.0.0.1:17388/authorize").is_err());
    }

    #[test]
    fn callback_main_path_auth_code_info() {
        // 抓包固化主路径（desktop oauth.rs:579-580）：native_ide 302 回调携带
        // authCodeInfo + userInfo + loginTraceID + host（全部 URL 编码 JSON）
        let url = "http://127.0.0.1:17388/authorize\
            ?authCodeInfo=%7B%22AuthCode%22%3A%22ac-123%22%2C%22ExpireAt%22%3A%222026-10-17T00%3A00%3A00Z%22%7D\
            &userInfo=%7B%22UserID%22%3A%22u-777%22%2C%22ScreenName%22%3A%22%E6%95%AC%E6%96%87%22%7D\
            &loginTraceID=trace-1\
            &host=https%3A%2F%2Fapi.trae.com.cn";
        let cb = parse_callback(url).unwrap();
        assert_eq!(cb.auth_code.as_deref(), Some("ac-123"));
        assert_eq!(cb.user_id.as_deref(), Some("u-777"));
        assert_eq!(cb.user_name.as_deref(), Some("敬文"));
        assert_eq!(cb.login_trace_id.as_deref(), Some("trace-1"));
        assert_eq!(cb.host.as_deref(), Some("https://api.trae.com.cn"));
        // authCodeInfo 坏 JSON → 明确报错（不静默）
        assert!(parse_callback("http://x/?authCodeInfo=not-json").is_err());
        // authCodeInfo 缺 AuthCode 字段 → 明确报错
        assert!(parse_callback("http://x/?authCodeInfo=%7B%7D").is_err());
        // userInfo 单独出现（无 authCode/rt/code）→ 仍报缺参
        assert!(parse_callback(
            "http://x/?userInfo=%7B%22UserID%22%3A%22u%22%7D"
        )
        .is_err());
    }

    #[test]
    fn device_public_key_pem_roundtrip() {
        let id = ModuleIdentity::load_or_create(None).unwrap();
        let pem = device_public_key_pem(&id).unwrap();
        assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----"));
        assert!(pem.trim_end().ends_with("-----END PUBLIC KEY-----"));
        // base64 body present between the markers
        let body = pem
            .trim_start_matches("-----BEGIN PUBLIC KEY-----")
            .trim_end_matches("-----END PUBLIC KEY-----");
        assert!(body.contains('\n'));
    }

    #[test]
    fn device_proof_shape() {
        let id = ModuleIdentity::load_or_create(None).unwrap();
        let proof = device_proof(&id.private_key_pem, "/trae/api/v3/oauth/ExchangeToken", OAUTH_CLIENT_ID, "rt").unwrap();
        assert!(proof.get("Signature").unwrap().as_str().unwrap().len() >= 80);
        assert!(proof.get("Timestamp").unwrap().is_i64(), "Timestamp must be JSON int");
        assert_eq!(proof.get("Nonce").unwrap().as_str().unwrap().len(), 32);
        // deterministic for the same inputs except nonce/timestamp → just verify it signs again
        let proof2 = device_proof(&id.private_key_pem, "/trae/api/v3/oauth/ExchangeToken", OAUTH_CLIENT_ID, "rt").unwrap();
        assert_ne!(proof["Nonce"], proof2["Nonce"]);
    }

    #[test]
    fn lazy_refresh_gate() {
        let now = 1_000_000_000i64;
        let h = |n: i64| now + n * 3600;
        // JWT fresh (60h) + rt fresh (60h) → skip
        assert!(!lazy_refresh_needed(Some(h(60)), Some(h(60)), now));
        // JWT stale → refresh
        assert!(lazy_refresh_needed(Some(h(40)), Some(h(60)), now));
        // JWT fresh but rt near expiry → refresh
        assert!(lazy_refresh_needed(Some(h(60)), Some(h(40)), now));
        // unknown JWT expiry → refresh
        assert!(lazy_refresh_needed(None, Some(h(60)), now));
        // rt expiry unknown → rt alone never triggers (JWT fresh → skip)
        assert!(!lazy_refresh_needed(Some(h(60)), None, now));
    }
}
