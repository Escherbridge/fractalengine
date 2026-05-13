use serde::{Deserialize, Serialize};

/// The licensing model for a `.hexon` archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenseType {
    Free,
    Attribution,
    Paid,
}

/// License metadata embedded in a `.hexon` manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct License {
    pub license_type: LicenseType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spdx_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_verification_url: Option<String>,
    #[serde(default)]
    pub free_entries: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_key: Option<String>,
}

impl Default for License {
    fn default() -> Self {
        Self {
            license_type: LicenseType::Free,
            spdx_id: None,
            attribution: None,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: Vec::new(),
            encrypted_key: None,
        }
    }
}
