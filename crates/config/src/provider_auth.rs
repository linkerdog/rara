use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;

use anyhow::{Context, Result};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use crate::ConfigManager;

#[derive(Serialize, Deserialize)]
pub(crate) struct ApiCredential {
    #[serde(rename = "type")]
    kind: String,
    #[serde(serialize_with = "serialize_key", deserialize_with = "deserialize_key")]
    pub key: SecretString,
}

fn serialize_key<S: serde::Serializer>(
    key: &SecretString,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use secrecy::ExposeSecret;
    serializer.serialize_str(key.expose_secret())
}

fn deserialize_key<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SecretString, D::Error> {
    String::deserialize(deserializer).map(SecretString::from)
}

impl ConfigManager {
    pub(crate) fn load_provider_auth(&self) -> Result<BTreeMap<String, ApiCredential>> {
        match fs::read(self.path.with_file_name("provider-auth.json")) {
            Ok(content) => serde_json::from_slice(&content).map_err(|_| {
                anyhow::anyhow!("Invalid provider-auth.json; expected provider API credentials")
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(err) => Err(err).context("Cannot read provider credentials"),
        }
    }

    pub fn save_registry_api_key(&self, provider: &str, key: SecretString) -> Result<()> {
        let mut lock_options = OpenOptions::new();
        lock_options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            lock_options.mode(0o600);
        }
        let lock = lock_options.open(self.path.with_file_name("provider-auth.lock"))?;
        lock.lock().context("Cannot lock provider credentials")?;
        let mut credentials = self.load_provider_auth()?;
        credentials.insert(
            provider.to_string(),
            ApiCredential {
                kind: "api".into(),
                key,
            },
        );
        let content = serde_json::to_vec_pretty(&credentials)?;
        let path = self.path.with_file_name("provider-auth.json");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
        let mut file = options
            .open(&temporary)
            .context("Cannot save provider credentials")?;
        file.write_all(&content)?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        Ok(())
    }
}
