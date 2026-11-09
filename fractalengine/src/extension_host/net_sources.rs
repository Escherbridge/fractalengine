//! Extension network broker wiring: builds `fe_plugin::net::Net` over the host's roles, audit sink and a local credential file.
//! Seam E7 in fe-plugin/src/net/AGENTS.md; the credential-store decision is in fractalengine/src/AGENTS.md §extension-net.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fe_plugin::bus::{AuditSink, RoleSource};
use fe_plugin::net::{Credential, CredentialStore, Net, NetSources};
use fe_plugin::tick::Clock;
use serde::{Deserialize, Serialize};

/// File name inside the extension data directory.
pub const CREDENTIALS_FILE: &str = "extension_credentials.json";
/// Larger files are treated as empty (a corrupt or hostile file must not be slurped on every connect).
const MAX_CREDENTIAL_FILE_BYTES: u64 = 64 * 1024;

/// One slot as the operator writes it.
#[derive(Clone, Serialize, Deserialize)]
struct StoredSlot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    secret: String,
}

/// Operator-filled slots in a local JSON file (`{ "<slot>": { "username"?, "secret" } }`): never replicated, re-read per connect.
pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    /// The store at `<data_dir>/extension_credentials.json`; a missing file means every slot is empty.
    pub fn new(data_dir: &Path) -> Self {
        Self { path: data_dir.join(CREDENTIALS_FILE) }
    }

    fn read_all(&self) -> BTreeMap<String, StoredSlot> {
        let Ok(meta) = std::fs::metadata(&self.path) else { return BTreeMap::new() };
        if meta.len() > MAX_CREDENTIAL_FILE_BYTES {
            tracing::warn!("extension credential file exceeds {MAX_CREDENTIAL_FILE_BYTES} bytes; treating every slot as empty");
            return BTreeMap::new();
        }
        let Ok(text) = std::fs::read_to_string(&self.path) else { return BTreeMap::new() };
        // The parse error text can echo file content, so only the fact of failure is logged.
        serde_json::from_str(&text).unwrap_or_else(|_| {
            tracing::warn!("extension credential file is not valid JSON; treating every slot as empty");
            BTreeMap::new()
        })
    }

    /// Fill or replace one slot (operator action); written atomically via a temp file and rename.
    pub fn set_slot(&self, slot: &str, username: Option<String>, secret: String) -> std::io::Result<()> {
        let mut all = self.read_all();
        all.insert(slot.to_owned(), StoredSlot { username, secret });
        let json = serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, &self.path)
    }
}

impl CredentialStore for FileCredentialStore {
    fn get(&self, slot: &str) -> Option<Credential> {
        let stored = self.read_all().remove(slot)?;
        if stored.secret.is_empty() {
            return None;
        }
        Some(Credential { username: stored.username, secret: stored.secret })
    }
}

/// The broker over the host's injected sources; `runtime` is the current Tokio runtime (None outside one: no dial-outs, no egress).
pub fn build_net(
    clock: Arc<dyn Clock>,
    roles: Arc<dyn RoleSource>,
    audit: Arc<dyn AuditSink>,
    extension_data_dir: &Path,
) -> Net {
    let credentials: Arc<dyn CredentialStore> = Arc::new(FileCredentialStore::new(extension_data_dir));
    let runtime = tokio::runtime::Handle::try_current().ok();
    Net::new(clock, NetSources { roles, audit, credentials, runtime })
}
