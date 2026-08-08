use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use codex_http_client::{HttpClientFactory, OutboundProxyPolicy};
use codex_login::{
    AuthCredentialsStoreMode, AuthDotJson, AuthKeyringBackendKind, AuthRouteConfig, CLIENT_ID,
    DeviceCode as CodexDeviceCode, LoginServer as CodexLoginServer, ServerOptions,
    complete_device_code_login as codex_complete_device_code_login, load_auth_dot_json,
    login_with_api_key as codex_login_with_api_key, logout as codex_logout,
    request_device_code as codex_request_device_code, run_login_server as codex_run_login_server,
};
use secrecy::SecretString;

const ISSUER: &str = "https://auth.openai.com";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedCodexAuthMode {
    ApiKey,
    Chatgpt,
}

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub verification_url: String,
    pub user_code: String,
    inner: CodexDeviceCode,
}

pub struct BrowserLoginSession {
    auth_url: String,
    inner: CodexLoginServer,
}

impl BrowserLoginSession {
    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }

    pub async fn complete(self, manager: &OAuthManager) -> Result<SecretString> {
        self.inner.block_until_done().await?;
        manager.load_saved_credential()
    }
}

#[derive(Clone)]
pub struct OAuthManager {
    codex_home: PathBuf,
    legacy_codex_home: PathBuf,
    saved_auth_available: Arc<Mutex<Option<bool>>>,
}

impl OAuthManager {
    pub fn new() -> Result<Self> {
        let config_dir = rara_config::ensure_rara_home_dir()?;
        Self::new_for_config_dir(config_dir)
    }

    pub fn new_for_config_dir(config_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&config_dir)?;
        let codex_home = preferred_codex_home(&config_dir);
        let legacy_codex_home = config_dir.join("codex-auth");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::create_dir_all(&legacy_codex_home)?;
        Ok(Self {
            codex_home,
            legacy_codex_home,
            saved_auth_available: Arc::new(Mutex::new(None)),
        })
    }

    pub fn start_browser_login(&self, open_browser: bool) -> Result<BrowserLoginSession> {
        let mut options = self.server_options(open_browser);
        options.port = 0;
        let session = codex_run_login_server(options)?;
        Ok(BrowserLoginSession {
            auth_url: session.auth_url.clone(),
            inner: session,
        })
    }

    pub async fn request_device_code(&self) -> Result<DeviceCode> {
        let options = self.server_options(false);
        let code = codex_request_device_code(&options).await?;
        Ok(DeviceCode {
            verification_url: code.verification_url.clone(),
            user_code: code.user_code.clone(),
            inner: code,
        })
    }

    pub async fn complete_device_code_login(
        &self,
        device_code: &DeviceCode,
    ) -> Result<SecretString> {
        let options = self.server_options(false);
        codex_complete_device_code_login(options, device_code.inner.clone()).await?;
        self.load_saved_credential()
    }

    pub fn save_api_key(&self, api_key: &str) -> Result<SecretString> {
        codex_login_with_api_key(
            &self.codex_home,
            api_key,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )?;
        self.set_saved_auth_cache(true);
        self.load_saved_credential()
    }

    pub fn clear_saved_auth(&self) -> Result<bool> {
        let mut removed = codex_logout(
            &self.codex_home,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )?;
        if self.legacy_codex_home != self.codex_home {
            removed |= codex_logout(
                &self.legacy_codex_home,
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
            )?;
        }
        self.clear_saved_auth_cache();
        Ok(removed)
    }

    pub fn has_saved_auth(&self) -> Result<bool> {
        if let Some(cached) = self.saved_auth_cache() {
            return Ok(cached);
        }
        for home in self.auth_homes_in_read_order() {
            let Some(auth) = load_auth_dot_json(
                home,
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
            )?
            else {
                continue;
            };
            let has_api_key = auth
                .openai_api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let has_access_token = auth
                .tokens
                .as_ref()
                .is_some_and(|tokens| !tokens.access_token.trim().is_empty());
            if has_api_key || has_access_token {
                self.set_saved_auth_cache(true);
                return Ok(true);
            }
        }
        self.set_saved_auth_cache(false);
        Ok(false)
    }

    pub fn read_api_key_from_stdin(&self) -> Result<SecretString> {
        eprintln!("Paste the Codex API key, then press Ctrl-D:");
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("API key input was empty"));
        }
        Ok(SecretString::from(trimmed.to_string()))
    }

    pub fn saved_auth_mode(&self) -> Result<Option<SavedCodexAuthMode>> {
        for home in self.auth_homes_in_read_order() {
            let Some(auth) = load_auth_dot_json(
                home,
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
            )?
            else {
                continue;
            };
            if let Some(mode) = detect_saved_auth_mode(&auth) {
                return Ok(Some(mode));
            }
        }
        Ok(None)
    }

    fn server_options(&self, open_browser: bool) -> ServerOptions {
        let mut options = ServerOptions::new(
            self.codex_home.clone(),
            CLIENT_ID.to_string(),
            None,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
            AuthRouteConfig::from_http_client_factory(HttpClientFactory::new(
                OutboundProxyPolicy::ReqwestDefault,
            )),
        );
        options.issuer = ISSUER.to_string();
        options.open_browser = open_browser;
        options
    }

    pub fn load_saved_credential(&self) -> Result<SecretString> {
        for home in self.auth_homes_in_read_order() {
            let Some(auth) = load_auth_dot_json(
                home,
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
            )?
            else {
                continue;
            };
            match detect_saved_auth_mode(&auth) {
                Some(SavedCodexAuthMode::ApiKey) => {
                    if let Some(api_key) =
                        auth.openai_api_key.filter(|value| !value.trim().is_empty())
                    {
                        self.set_saved_auth_cache(true);
                        return Ok(SecretString::from(api_key));
                    }
                }
                Some(SavedCodexAuthMode::Chatgpt) => {
                    if let Some(tokens) = auth
                        .tokens
                        .filter(|tokens| !tokens.access_token.trim().is_empty())
                    {
                        self.set_saved_auth_cache(true);
                        return Ok(SecretString::from(tokens.access_token));
                    }
                }
                None => {}
            }
        }
        Err(anyhow!(
            "Codex login finished but auth storage did not contain an API key or access token"
        ))
    }

    pub fn invalidate_saved_auth_cache(&self) {
        self.clear_saved_auth_cache();
    }

    /// Reserved for Codex model catalog refresh wiring (docs/todo.md).
    #[allow(dead_code)]
    pub fn codex_home(&self) -> &Path {
        self.codex_home.as_path()
    }

    fn auth_homes_in_read_order(&self) -> [&Path; 2] {
        [self.codex_home.as_path(), self.legacy_codex_home.as_path()]
    }

    fn saved_auth_cache(&self) -> Option<bool> {
        self.saved_auth_available
            .lock()
            .ok()
            .and_then(|guard| *guard)
    }

    fn set_saved_auth_cache(&self, value: bool) {
        if let Ok(mut guard) = self.saved_auth_available.lock() {
            *guard = Some(value);
        }
    }

    fn clear_saved_auth_cache(&self) {
        if let Ok(mut guard) = self.saved_auth_available.lock() {
            *guard = None;
        }
    }
}

fn preferred_codex_home(config_dir: &Path) -> PathBuf {
    config_dir
        .parent()
        .map(|parent| parent.join(".codex"))
        .unwrap_or_else(|| config_dir.join(".codex"))
}

fn detect_saved_auth_mode(auth: &AuthDotJson) -> Option<SavedCodexAuthMode> {
    let explicit_mode = auth.auth_mode.as_ref().map(|mode| format!("{mode:?}"));
    if explicit_mode.as_deref() == Some("ApiKey") {
        return Some(SavedCodexAuthMode::ApiKey);
    }
    if explicit_mode
        .as_deref()
        .is_some_and(|mode| mode.starts_with("Chatgpt"))
    {
        return Some(SavedCodexAuthMode::Chatgpt);
    }
    if auth
        .tokens
        .as_ref()
        .is_some_and(|tokens| !tokens.access_token.trim().is_empty())
    {
        return Some(SavedCodexAuthMode::Chatgpt);
    }
    if auth
        .openai_api_key
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Some(SavedCodexAuthMode::ApiKey);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use secrecy::ExposeSecret;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn browser_login_session_uses_codex_issuer_and_client_id() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");
        let session = manager
            .start_browser_login(false)
            .expect("browser login session");

        assert!(
            session
                .auth_url()
                .starts_with("https://auth.openai.com/oauth/authorize?")
        );
        assert!(session.auth_url().contains(CLIENT_ID));
        session.inner.cancel();
    }

    #[test]
    fn save_api_key_persists_via_codex_auth_storage() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        let stored = manager.save_api_key("sk-test-123").expect("save api key");

        assert_eq!(stored.expose_secret(), "sk-test-123");

        let auth = load_auth_dot_json(
            auth_path(&manager),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("load auth")
        .expect("auth file");
        assert_eq!(auth.openai_api_key.as_deref(), Some("sk-test-123"));
        assert!(auth.tokens.is_none());
    }

    #[test]
    fn load_saved_credential_prefers_access_token_for_chatgpt_auth() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        codex_login::save_auth(
            auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: Some("sk-direct".into()),
                tokens: Some(codex_login::TokenData {
                    id_token: valid_id_token_info(),
                    access_token: "access".into(),
                    refresh_token: "refresh".into(),
                    account_id: None,
                }),
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save auth");

        let saved = manager.load_saved_credential().expect("load chatgpt token");
        assert_eq!(saved.expose_secret(), "access");
        assert_eq!(
            manager.saved_auth_mode().expect("auth mode"),
            Some(SavedCodexAuthMode::Chatgpt)
        );

        codex_login::save_auth(
            auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: None,
                tokens: Some(codex_login::TokenData {
                    id_token: valid_id_token_info(),
                    access_token: "access-only".into(),
                    refresh_token: "refresh".into(),
                    account_id: None,
                }),
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save token auth");

        let saved = manager.load_saved_credential().expect("load access token");
        assert_eq!(saved.expose_secret(), "access-only");
    }

    #[test]
    fn load_saved_credential_uses_api_key_for_api_key_auth() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        codex_login::save_auth(
            auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: Some("sk-direct".into()),
                tokens: None,
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save auth");

        let saved = manager.load_saved_credential().expect("load api key");
        assert_eq!(saved.expose_secret(), "sk-direct");
        assert_eq!(
            manager.saved_auth_mode().expect("auth mode"),
            Some(SavedCodexAuthMode::ApiKey)
        );
    }

    #[test]
    fn logout_clears_codex_auth_storage() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");
        manager
            .save_api_key("sk-test-logout")
            .expect("save api key");

        let removed = manager.clear_saved_auth().expect("clear auth");
        assert!(removed);

        let auth = load_auth_dot_json(
            auth_path(&manager),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("load auth after logout");
        assert!(auth.is_none());
    }

    #[test]
    fn has_saved_auth_detects_api_key_and_access_token_storage() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        assert!(!manager.has_saved_auth().expect("no auth"));

        manager.save_api_key("sk-test-123").expect("save api key");
        assert!(manager.has_saved_auth().expect("api key auth"));

        manager.clear_saved_auth().expect("clear auth");
        assert!(!manager.has_saved_auth().expect("cleared auth"));

        codex_login::save_auth(
            auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: None,
                tokens: Some(codex_login::TokenData {
                    id_token: valid_id_token_info(),
                    access_token: "access-only".into(),
                    refresh_token: "refresh".into(),
                    account_id: None,
                }),
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save token auth");

        manager.invalidate_saved_auth_cache();
        assert!(manager.has_saved_auth().expect("token auth"));
    }

    #[test]
    fn load_saved_credential_rejects_blank_values() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        codex_login::save_auth(
            auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: Some("   ".into()),
                tokens: Some(codex_login::TokenData {
                    id_token: valid_id_token_info(),
                    access_token: "   ".into(),
                    refresh_token: "refresh".into(),
                    account_id: None,
                }),
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save auth");

        let err = manager
            .load_saved_credential()
            .expect_err("blank credentials should be rejected");
        assert!(
            err.to_string()
                .contains("did not contain an API key or access token")
        );
    }

    #[test]
    fn has_saved_auth_refreshes_after_cache_invalidation() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        assert!(!manager.has_saved_auth().expect("no auth"));

        codex_login::save_auth(
            auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: Some("sk-direct".into()),
                tokens: None,
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save auth");

        assert!(!manager.has_saved_auth().expect("stale false cache"));
        manager.invalidate_saved_auth_cache();
        assert!(manager.has_saved_auth().expect("refreshed auth"));
    }

    #[test]
    fn load_saved_credential_prefers_official_codex_home_over_legacy_fallback() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        codex_login::save_auth(
            legacy_auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: Some("sk-legacy".into()),
                tokens: None,
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save legacy auth");
        codex_login::save_auth(
            auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: Some("sk-official".into()),
                tokens: None,
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save official auth");

        let saved = manager
            .load_saved_credential()
            .expect("load preferred auth");
        assert_eq!(saved.expose_secret(), "sk-official");
    }

    #[test]
    fn load_saved_credential_falls_back_to_legacy_codex_home() {
        let temp = tempdir().expect("tempdir");
        let manager =
            OAuthManager::new_for_config_dir(temp.path().join(".rara")).expect("oauth manager");

        codex_login::save_auth(
            legacy_auth_path(&manager),
            &codex_login::AuthDotJson {
                auth_mode: None,
                openai_api_key: Some("sk-legacy".into()),
                tokens: None,
                last_refresh: None,
                agent_identity: None,
                personal_access_token: None,
                bedrock_api_key: None,
            },
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save legacy auth");

        let saved = manager.load_saved_credential().expect("load legacy auth");
        assert_eq!(saved.expose_secret(), "sk-legacy");
    }

    fn auth_path(manager: &OAuthManager) -> &Path {
        manager.codex_home.as_path()
    }

    fn legacy_auth_path(manager: &OAuthManager) -> &Path {
        manager.legacy_codex_home.as_path()
    }

    fn valid_id_token_info() -> codex_login::token_data::IdTokenInfo {
        codex_login::token_data::parse_chatgpt_jwt_claims("eyJhbGciOiJub25lIn0.e30.signature")
            .expect("valid id token")
    }
}
