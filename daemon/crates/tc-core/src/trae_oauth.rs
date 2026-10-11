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
}

/// Parse the loopback callback URL:
/// `http://127.0.0.1:port/authorize?refreshToken=…&accessToken=…&userId=…`
/// (or a `code=` variant that requires exchange_token_auth_code).
pub fn parse_callback(callback_url: &str) -> Result<CallbackInfo, String> {
    let query = callback_url.split('?').nth(1).ok_or("回调 URL 中缺少查询参数")?;
    let mut params = std::collections::HashMap::new();
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let k = kv.next().unwrap_or("").to_string();
        let v = kv.next().unwrap_or("").to_string();
        params.insert(k, url_decode(&v));
    }
    let rt = params
        .get("refreshToken")
        .or_else(|| params.get("refresh_token"))
        .cloned()
        .unwrap_or_default();
    let code = params.get("code").cloned().unwrap_or_default();
    if rt.is_empty() && code.is_empty() {
        return Err("回调 URL 缺少 refreshToken 或 code 参数".into());
    }
    Ok(CallbackInfo {
        refresh_token: rt,
        access_token: params.get("accessToken").or_else(|| params.get("access_token")).cloned(),
        user_id: params
            .get("userId")
            .or_else(|| params.get("user_id"))
            .or_else(|| params.get("UserID"))
            .cloned(),
        user_name: params.get("userName").or_else(|| params.get("username")).cloned(),
        login_trace_id: params.get("loginTraceID").or_else(|| params.get("login_trace_id")).cloned(),
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

/// AuthCode exchange (callback `code=` variant): DeviceInfo variant, no proof.
pub fn exchange_token_auth_code(
    agent: &ureq::Agent,
    identity: &ModuleIdentity,
    auth_code: &str,
    code_verifier: &str,
) -> Result<ExchangeResult, String> {
    let url = "https://api.trae.com.cn/trae/api/v3/oauth/ExchangeToken";
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-device-id".to_string(), identity.device_id.clone()),
    ];
    let payload = json!({
        "ClientID": OAUTH_CLIENT_ID,
        "AuthCode": auth_code,
        "CodeVerifier": code_verifier,
        "DeviceInfo": {
            "DeviceID": identity.device_id,
            "MachineID": identity.machine_id,
        },
        "IDEVersion": OAUTH_PAGE_APP_VERSION,
    });
    let (status, body, raw) = http::post_json(agent, &url, &headers, &payload);
    if status == 0 {
        return Err("ExchangeToken 网络不可达".into());
    }
    let b = body.unwrap_or(json!({}));
    let acc = dig(&b, &["accessToken", "access_token"])
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if acc.is_empty() {
        return Err(format!("ExchangeToken(AuthCode) 响应缺少 accessToken（HTTP {status}）: {}", truncate(&raw, 160)));
    }
    Ok(ExchangeResult {
        access_token: acc,
        refresh_token: dig(&b, &["refreshToken", "refresh_token"])
            .and_then(|v| v.as_str())
            .map(String::from),
        body: b,
    })
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
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
