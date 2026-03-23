/// `ModelUrlMeta` is a Bevy `Component` that stores optional external and config URLs
/// for model entities in the FractalEngine scene graph.
///
/// Both fields are `Option<String>` and default to `None`, so existing scene files
/// that omit these fields will still deserialize without error.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    bevy::prelude::Component,
    bevy::prelude::Reflect,
)]
#[reflect(Component, Default)]
pub struct ModelUrlMeta {
    /// The external URL to open in the Petal Portal overlay when this model is selected.
    #[serde(default)]
    pub external_url: Option<String>,
    /// The config URL, only accessible to users with the Admin role.
    #[serde(default)]
    pub config_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_url_meta_defaults_to_none() {
        let meta = ModelUrlMeta::default();
        assert!(meta.external_url.is_none());
        assert!(meta.config_url.is_none());
    }

    #[test]
    fn model_url_meta_roundtrips_serde() {
        let meta = ModelUrlMeta {
            external_url: Some("https://example.com/dashboard".to_string()),
            config_url: Some("https://admin.example.com/config".to_string()),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: ModelUrlMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta.external_url, back.external_url);
        assert_eq!(meta.config_url, back.config_url);
    }

    #[test]
    fn model_url_meta_deserializes_missing_fields_as_none() {
        let json = "{}";
        let meta: ModelUrlMeta = serde_json::from_str(json).unwrap();
        assert!(meta.external_url.is_none());
        assert!(meta.config_url.is_none());
    }
}
