//! SPEC-10 §7.1 observation source classes and the §2.2 rule 4 `format_id` grammar: the ONE
//! definition every crate reaches. Rationale: `src/AGENTS.md` §source-class.

use serde::{Deserialize, Serialize};

/// Who produced an observation (SPEC-10 §7.1 key 0). Carried explicitly from caller to door.
///
/// Deliberately no `Default`: a defaulted class is an inferred class, which §7.1 rule 6 forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceClass {
    /// A field device authenticating with its own token (wire `1`).
    Device,
    /// A configured connector relaying another system's data (wire `2`).
    Connector,
    /// A sandboxed decoder plugin that read a device payload (wire `3`).
    Decoder,
    /// A person entering an observation by hand (wire `4`).
    Manual,
    /// The simulation lab; never presented as field data (wire `5`).
    Simulation,
}

impl SourceClass {
    /// Every class in wire order.
    pub const ALL: [SourceClass; 5] = [
        SourceClass::Device,
        SourceClass::Connector,
        SourceClass::Decoder,
        SourceClass::Manual,
        SourceClass::Simulation,
    ];

    /// The provisional SPEC-10 §7.1 wire value; zero is never a class.
    pub const fn wire(self) -> u64 {
        match self {
            Self::Device => 1,
            Self::Connector => 2,
            Self::Decoder => 3,
            Self::Manual => 4,
            Self::Simulation => 5,
        }
    }

    /// Decodes a wire value, refusing `0` and everything above `5` (§7.1 rule 2).
    pub const fn from_wire(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::Device),
            2 => Some(Self::Connector),
            3 => Some(Self::Decoder),
            4 => Some(Self::Manual),
            5 => Some(Self::Simulation),
            _ => None,
        }
    }

    /// The snake_case name the serde form and logs use.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Connector => "connector",
            Self::Decoder => "decoder",
            Self::Manual => "manual",
            Self::Simulation => "simulation",
        }
    }
}

impl std::fmt::Display for SourceClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Shortest legal `format_id`, in bytes (SPEC-10 §2.2 rule 4).
pub const FORMAT_ID_MIN_BYTES: usize = 5;
/// Longest legal `format_id`, in bytes (SPEC-10 §2.2 rule 4).
pub const FORMAT_ID_MAX_BYTES: usize = 128;

/// Whether `id` satisfies the SPEC-10 §2.2 rule 4 grammar `<namespace>/<name>@<major>`.
///
/// `<namespace>` is two or more dot-separated `[a-z0-9][a-z0-9-]*` labels, `<name>` is
/// `[a-z0-9][a-z0-9._-]*`, and `<major>` is `0` or a decimal without a leading zero that fits
/// in a `u32`. The whole id is lowercase ASCII of 5 to 128 bytes. It enters signed bytes, so
/// every descriptor, ingest door and canonical codec MUST call this one function.
pub fn is_valid_format_id(id: &str) -> bool {
    if !(FORMAT_ID_MIN_BYTES..=FORMAT_ID_MAX_BYTES).contains(&id.len()) {
        return false;
    }
    let Some((namespace, rest)) = id.split_once('/') else {
        return false;
    };
    let Some((name, major)) = rest.split_once('@') else {
        return false;
    };
    is_valid_namespace(namespace) && is_valid_name(name) && is_valid_major(major)
}

fn is_label_start(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

fn is_valid_namespace(namespace: &str) -> bool {
    let mut labels = 0usize;
    for label in namespace.split('.') {
        let bytes = label.as_bytes();
        let Some((&first, tail)) = bytes.split_first() else {
            return false;
        };
        if !is_label_start(first)
            || !tail
                .iter()
                .all(|&byte| is_label_start(byte) || byte == b'-')
        {
            return false;
        }
        labels += 1;
    }
    labels >= 2
}

fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some((&first, tail)) = bytes.split_first() else {
        return false;
    };
    is_label_start(first)
        && tail
            .iter()
            .all(|&byte| is_label_start(byte) || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_valid_major(major: &str) -> bool {
    if major.is_empty() || !major.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if major.len() > 1 && major.starts_with('0') {
        return false;
    }
    major.parse::<u32>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_round_trip_and_zero_or_unknown_is_never_a_class() {
        for class in SourceClass::ALL {
            assert_eq!(SourceClass::from_wire(class.wire()), Some(class));
        }
        for refused in [0u64, 6, 255, u64::MAX] {
            assert_eq!(SourceClass::from_wire(refused), None, "{refused}");
        }
    }

    #[test]
    fn serde_names_match_the_display_names() {
        for class in SourceClass::ALL {
            let json = serde_json::to_string(&class).expect("serialize");
            assert_eq!(json, format!("\"{}\"", class.as_str()));
            assert_eq!(
                serde_json::from_str::<SourceClass>(&json).expect("deserialize"),
                class
            );
        }
        assert!(serde_json::from_str::<SourceClass>("\"field\"").is_err());
    }

    #[test]
    fn format_id_grammar_table() {
        let longest_legal = format!("ab.cd/{}@1", "n".repeat(FORMAT_ID_MAX_BYTES - 8));
        assert_eq!(longest_legal.len(), FORMAT_ID_MAX_BYTES);
        let one_too_long = format!("ab.cd/{}@1", "n".repeat(FORMAT_ID_MAX_BYTES - 7));
        let cases: &[(&str, bool)] = &[
            ("acme.thermo/frame@1", true),
            ("a.b/c@0", true),
            ("acme-co.sensors-2/frame_v.x-1@4294967295", true),
            (&longest_legal, true),
            (&one_too_long, false),
            ("acme/frame@1", false),
            ("acme..thermo/frame@1", false),
            (".acme.thermo/frame@1", false),
            ("acme.-thermo/frame@1", false),
            ("Acme.thermo/frame@1", false),
            ("acme.thermo/Frame@1", false),
            ("acme.thermo/_frame@1", false),
            ("acme.thermo/fr/ame@1", false),
            ("acme.thermo/frame", false),
            ("acme.thermo/frame@", false),
            ("acme.thermo/frame@01", false),
            ("acme.thermo/frame@4294967296", false),
            ("acme.thermo/frame@1@2", false),
            ("acme.thermo/frame@-1", false),
            ("acme.thermo/fráme@1", false),
            ("acme.thermo@1", false),
            ("", false),
        ];
        for (id, expected) in cases {
            assert_eq!(is_valid_format_id(id), *expected, "{id:?}");
        }
    }
}
