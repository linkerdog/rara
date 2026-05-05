//! Google OAuth PKCE flow for Gemini Code Assist.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::Url;

// ── Constants ─────────────────────────────────────────────────

/// Google OAuth client ID. Set `RARA_GOOGLE_CLIENT_ID` in the
/// environment, or use the public gemini-cli desktop OAuth client
/// shipped in Google's open-source gemini-cli repository.
fn google_oauth_client_id() -> &'static str {
    let s = std::env::var("RARA_GOOGLE_CLIENT_ID").unwrap_or_else(|_| {
        unimplemented!(
            "RARA_GOOGLE_CLIENT_ID env var not set. \
             Set it to your Google OAuth client ID."
        )
    });
    Box::leak(s.into_boxed_str())
}

fn google_oauth_client_secret() -> &'static str {
    let s = std::env::var("RARA_GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| {
        unimplemented!(
            "RARA_GOOGLE_CLIENT_SECRET env var not set. \
             Set it to your Google OAuth client secret."
        )
    });
    Box::leak(s.into_boxed_str())
}

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REDIRECT_URI_PREFIX: &str = "http://127.0.0.1:";
const GOOGLE_USERINFO_URL: &str =
    "https://openidconnect.googleapis.com/v1/userinfo";
const OAUTH_SCOPES: &str =
    "openid email https://www.googleapis.com/auth/cloud-platform";
const DEFAULT_DEVICE_CODE_URL: &str =
    "https://oauth2.googleapis.com/device/code";
const DEFAULT_DEVICE_CODE_GRANT_URL: &str =
    "https://oauth2.googleapis.com/token";

// ── Data types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    managed_project_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct StoredCredential {
    refresh: String,
    access: String,
    expires: u64,
    email: String,
}

#[derive(Debug, Clone)]
pub struct GoogleCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub email: String,
    pub project_id: Option<String>,
    pub managed_project_id: Option<String>,
}

pub struct BrowserLoginSession {
    pub auth_url: String,
    pub(super) addr: SocketAddr,
    pub(super) code_verifier: String,
    pub(super) token_path: PathBuf,
}

#[derive(Debug)]
pub struct DeviceCodeSession {
    pub verification_url: String,
    pub user_code: String,
    pub(super) device_code: String,
    pub(super) interval_secs: u64,
    pub(super) expires_in_secs: u64,
    pub(super) code_verifier: String,
    pub(super) token_path: PathBuf,
}

#[derive(Clone)]
pub struct GoogleOAuthManager {
    token_path: PathBuf,
    http: reqwest::Client,
    credential: Arc<Mutex<Option<GoogleCredential>>>,
}

impl std::fmt::Debug for GoogleOAuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleOAuthManager")
            .field("token_path", &self.token_path)
            .field("http", &"reqwest::Client")
            .finish()
    }
}

// ── Manager impl ──────────────────────────────────────────────

impl GoogleOAuthManager {
    pub fn new(config_dir: PathBuf) -> Result<Self> {
        let auth_dir = config_dir.join("auth");
        std::fs::create_dir_all(&auth_dir)?;
        let token_path = auth_dir.join("google_oauth.json");
        Ok(Self {
            token_path,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            credential: Arc::new(Mutex::new(None)),
        })
    }

    pub fn has_saved_auth(&self) -> bool {
        self.token_path.exists()
    }

    pub fn clear_saved_auth(&self) -> anyhow::Result<bool> {
        if !self.token_path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&self.token_path)?;
        Ok(true)
    }

    pub async fn load_credential(&self) -> Result<GoogleCredential> {
        // check cache first
        if let Some(cached) = self.credential.lock().await.as_ref() {
            let now_ms = system_time_millis();
            if now_ms + 300_000 < cached.expires_at {
                return Ok(cached.clone());
            }
        }

        let data = std::fs::read_to_string(&self.token_path)
            .context("No saved Google OAuth credential")?;
        let stored: StoredCredential =
            serde_json::from_str(&data)
                .context("Failed to parse Google OAuth credential")?;

        let (refresh_token, project_id, managed_project_id) =
            unpack_refresh(&stored.refresh);
        let cred = GoogleCredential {
            access_token: stored.access,
            refresh_token,
            expires_at: stored.expires,
            email: stored.email,
            project_id,
            managed_project_id,
        };

        let cred = self.refresh_if_needed(cred).await?;
        *self.credential.lock().await = Some(cred.clone());
        Ok(cred)
    }

    pub fn start_browser_login(&self) -> Result<BrowserLoginSession> {
        let code_verifier = generate_code_verifier();
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let port = addr.port();
        // Close the listener immediately; reopen it in complete_browser_login.
        // This keeps TcpListener (non-Send on some platforms) out of the
        // BrowserLoginSession struct that travels across async boundaries.
        drop(listener);
        let redirect_uri = format!("{REDIRECT_URI_PREFIX}{port}");

        let auth_url = build_auth_url(&code_verifier, &redirect_uri)?;
        let _ = open::that(&auth_url);

        Ok(BrowserLoginSession {
            auth_url,
            addr,
            code_verifier,
            token_path: self.token_path.clone(),
        })
    }

    pub async fn complete_browser_login(
        session: BrowserLoginSession,
    ) -> Result<GoogleCredential> {
        let listener = TcpListener::bind(session.addr)?;
        listener
            .set_nonblocking(false)
            .context("Failed to set listener blocking")?;
        let code = receive_callback(listener)?;
        let token = exchange_authorization_code(
            &session.code_verifier,
            &code,
        )
        .await?;
        let cred =
            persist_credential(&session.token_path, &token).await?;
        Ok(cred)
    }

    pub async fn request_device_code(
        &self,
    ) -> Result<DeviceCodeSession> {
        let code_verifier = generate_code_verifier();
        let code_challenge = compute_code_challenge(&code_verifier);

        let params = [
            ("client_id", google_oauth_client_id()),
            ("scope", OAUTH_SCOPES),
            ("code_challenge", &code_challenge),
            ("code_challenge_method", "S256"),
        ];

        let url =
            Url::parse_with_params(DEFAULT_DEVICE_CODE_URL, &params)?;
        let resp: serde_json::Value = self
            .http
            .post(url)
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded",
            )
            .send()
            .await?
            .json()
            .await?;

        Ok(DeviceCodeSession {
            verification_url: resp["verification_url"]
                .as_str()
                .unwrap_or("https://www.google.com/device")
                .to_string(),
            user_code: resp["user_code"]
                .as_str()
                .context("missing user_code")?
                .to_string(),
            device_code: resp["device_code"]
                .as_str()
                .context("missing device_code")?
                .to_string(),
            interval_secs: resp["interval"].as_u64().unwrap_or(5),
            expires_in_secs: resp["expires_in"].as_u64().unwrap_or(600),
            code_verifier,
            token_path: self.token_path.clone(),
        })
    }

    pub async fn complete_device_code(
        session: DeviceCodeSession,
    ) -> Result<GoogleCredential> {
        let deadline = system_time_millis()
            + session.expires_in_secs * 1000;
        let interval =
            Duration::from_secs(session.interval_secs.min(30));

        loop {
            if system_time_millis() >= deadline {
                bail!("Device code authorization timed out");
            }

            let form_body = device_code_form_body(&session);
            let resp = reqwest::Client::new()
                .post(DEFAULT_DEVICE_CODE_GRANT_URL)
                .header(
                    "Content-Type",
                    "application/x-www-form-urlencoded",
                )
                .body(form_body)
                .send()
                .await?;

            if resp.status().is_success() {
                let token: TokenResponse = resp.json().await?;
                return persist_credential(
                    &session.token_path,
                    &token,
                )
                .await;
            }

            if resp.status().as_u16() == 400 {
                let body: serde_json::Value =
                    resp.json().await?;
                let error = body["error"]
                    .as_str()
                    .unwrap_or("unknown");
                if error == "authorization_pending" {
                    tokio::time::sleep(interval).await;
                    continue;
                }
                if error == "slow_down" {
                    tokio::time::sleep(
                        Duration::from_secs(
                            session.interval_secs.min(10) + 5,
                        ),
                    )
                    .await;
                    continue;
                }
                bail!("Device code error: {error}");
            }

            bail!(
                "Unexpected device code response: {}",
                resp.status()
            );
        }
    }

    // ── Token refresh ──────────────────────────────────────

    async fn refresh_if_needed(
        &self,
        cred: GoogleCredential,
    ) -> Result<GoogleCredential> {
        let now_ms = system_time_millis();
        if now_ms + 300_000 >= cred.expires_at {
            return self.refresh_access_token(&cred).await;
        }
        Ok(cred)
    }

    async fn refresh_access_token(
        &self,
        cred: &GoogleCredential,
    ) -> Result<GoogleCredential> {
        let form_body = urlencode_pairs(&[
            ("client_id", google_oauth_client_id()),
            ("client_secret", google_oauth_client_secret()),
            ("refresh_token", &cred.refresh_token),
            ("grant_type", "refresh_token"),
        ]);

        let resp = self
            .http
            .post(GOOGLE_TOKEN_URL)
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded",
            )
            .body(form_body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("Failed to refresh token: {body}");
        }

        let token: TokenResponse = resp.json().await?;
        let expires_at = system_time_millis()
            + token.expires_in.unwrap_or(3600) * 1000;

        Ok(GoogleCredential {
            access_token: token.access_token,
            refresh_token: token
                .refresh_token
                .unwrap_or_else(|| cred.refresh_token.clone()),
            expires_at,
            email: cred.email.clone(),
            project_id: cred.project_id.clone(),
            managed_project_id: cred.managed_project_id.clone(),
        })
    }
}

// ── PKCE helpers ──────────────────────────────────────────────

fn generate_code_verifier() -> String {
    let bytes: [u8; 64] = rand::random();
    URL_SAFE_NO_PAD.encode(&bytes)
}

fn compute_code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(&digest)
}

fn build_auth_url(
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<String> {
    let code_challenge = compute_code_challenge(code_verifier);
    let state = generate_state();

    let mut url = Url::parse(GOOGLE_AUTH_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", google_oauth_client_id())
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", OAUTH_SCOPES)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    Ok(url.to_string())
}

fn generate_state() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(&bytes)
}

// ── HTTP callback server ──────────────────────────────────────

fn receive_callback(listener: TcpListener) -> Result<String> {
    let mut stream = listener
        .incoming()
        .next()
        .context("No incoming connection")??;

    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let code = request
        .lines()
        .next()
        .and_then(|line| {
            let path = line.split_whitespace().nth(1)?;
            let query = path.split('?').nth(1)?;
            Url::parse(&format!("http://localhost/?{query}"))
                .ok()?
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.into_owned())
        })
        .context("No authorization code in callback")?;

    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
        <html><body><h1>RARA \u{2014} Google OAuth Complete</h1>\
        <p>You can close this window and return to the terminal.</p></body></html>";
    let _ = stream.write_all(response.as_bytes());

    Ok(code)
}

// ── Token exchange ────────────────────────────────────────────

async fn exchange_authorization_code(
    code_verifier: &str,
    code: &str,
) -> Result<TokenResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    let form_body = urlencode_pairs(&[
        ("client_id", google_oauth_client_id()),
        ("client_secret", google_oauth_client_secret()),
        ("code", code),
        ("code_verifier", code_verifier),
        ("grant_type", "authorization_code"),
    ]);

    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .header(
            "Content-Type",
            "application/x-www-form-urlencoded",
        )
        .body(form_body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("Token exchange failed: {body}");
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token)
}

fn device_code_form_body(session: &DeviceCodeSession) -> String {
    urlencode_pairs(&[
        ("client_id", google_oauth_client_id()),
        ("client_secret", google_oauth_client_secret()),
        ("device_code", &session.device_code),
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:device_code",
        ),
        ("code_verifier", &session.code_verifier),
    ])
}

// ── Persistence ───────────────────────────────────────────────

async fn persist_credential(
    path: &Path,
    token: &TokenResponse,
) -> Result<GoogleCredential> {
    let refresh_token = token
        .refresh_token
        .clone()
        .context("No refresh token in response")?;
    let expires_in = token.expires_in.unwrap_or(3600);
    let expires_at = system_time_millis() + expires_in * 1000;

    // Resolve email and project info.
    let email = resolve_email(&token.access_token).await?;
    let packed_refresh = pack_refresh(
        &refresh_token,
        token.project_id.as_deref(),
        token.managed_project_id.as_deref(),
    );

    let stored = StoredCredential {
        refresh: packed_refresh,
        access: token.access_token.clone(),
        expires: expires_at,
        email: email.clone(),
    };

    let json = serde_json::to_string_pretty(&stored)?;
    std::fs::write(path, json)?;

    // Set restrictive permissions on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }

    Ok(GoogleCredential {
        access_token: token.access_token.clone(),
        refresh_token,
        expires_at,
        email,
        project_id: token.project_id.clone(),
        managed_project_id: token.managed_project_id.clone(),
    })
}

async fn resolve_email(access_token: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let resp: serde_json::Value = client
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await?
        .json()
        .await?;

    resp["email"]
        .as_str()
        .map(|s| s.to_string())
        .context("No email in userinfo response")
}

// ── Helpers ───────────────────────────────────────────────────

/// Build a URL-encoded form body from key-value pairs.
///
/// All OAuth parameter values are ASCII-safe (alphanumeric plus `-_.`).
fn urlencode_pairs(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn pack_refresh(
    refresh_token: &str,
    project_id: Option<&str>,
    managed_project_id: Option<&str>,
) -> String {
    let pid = project_id.unwrap_or("");
    let mpid = managed_project_id.unwrap_or("");
    format!("{refresh_token}|{pid}|{mpid}")
}

fn unpack_refresh(packed: &str) -> (String, Option<String>, Option<String>) {
    let parts: Vec<&str> = packed.splitn(3, '|').collect();
    let rt = parts.first().map(|s| s.to_string()).unwrap_or_default();
    let pid = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string());
    let mpid = parts.get(2).filter(|s| !s.is_empty()).map(|s| s.to_string());
    (rt, pid, mpid)
}

fn system_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
