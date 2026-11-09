//! Bundle origin labels and the one origin parser (protocol.md §1); see `portal_bundle/AGENTS.md` §origins.

use std::fmt;

/// Custom-protocol scheme registered with wry.
pub const SCHEME: &str = "fe-portal";
/// Windows document spelling prefix: wry rewrites `fe-portal://` to this (`with_https_scheme(true)`).
const HTTPS_PREFIX: &str = "https://fe-portal.";
/// Protocol-handler and non-Windows spelling prefix.
const CUSTOM_PREFIX: &str = "fe-portal://";
/// Every bundle host ends in this suffix (OD-1).
const HOST_SUFFIX: &str = ".localhost";
/// Base32 characters after the `a-` / `s-` prefix (130 bits).
const LABEL_HASH_CHARS: usize = 26;
/// RFC 4648 base32 alphabet, lowercase, no padding.
const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Which origin family a label belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelKind {
    /// `a-…`: one per (publisher, package); survives updates.
    Admitted,
    /// `s-…`: one per build (content hash); never shares storage with an admitted origin.
    Static,
}

/// A validated `(a|s)-[a-z2-7]{26}` origin label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Label(String);

impl Label {
    /// Accepts exactly `(a|s)-[a-z2-7]{26}`.
    pub fn parse(raw: &str) -> Option<Self> {
        let bytes = raw.as_bytes();
        let prefix_ok = bytes.len() == 2 + LABEL_HASH_CHARS
            && matches!(bytes[0], b'a' | b's')
            && bytes[1] == b'-';
        let body_ok = prefix_ok
            && bytes[2..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(c));
        body_ok.then(|| Self(raw.to_owned()))
    }

    /// The label text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Admitted or static family.
    pub fn kind(&self) -> LabelKind {
        if self.0.starts_with("a-") {
            LabelKind::Admitted
        } else {
            LabelKind::Static
        }
    }

    /// `https://fe-portal.<label>.localhost`: the origin the browser keys storage on.
    pub fn https_origin(&self) -> String {
        format!("{HTTPS_PREFIX}{}{HOST_SUFFIX}", self.0)
    }

    /// Document URL for a `web/` member (protocol.md §1.7); `None` for any other path.
    pub fn document_url(&self, member: &str) -> Option<String> {
        let rest = member.strip_prefix("web/")?;
        Some(format!("{}/{rest}", self.https_origin()))
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Admitted label: `a-` + b32(BLAKE3(publisher_did ‖ 0x00 ‖ package_id))[..26].
pub fn admitted_label(publisher_did: &str, package_id: &str) -> Label {
    let mut hasher = blake3::Hasher::new();
    hasher.update(publisher_did.as_bytes());
    hasher.update(&[0u8]);
    hasher.update(package_id.as_bytes());
    Label(format!("a-{}", base32_prefix(hasher.finalize().as_bytes(), LABEL_HASH_CHARS)))
}

/// Static label: `s-` + b32(content_hash bytes)[..26]; `None` unless `content_hash` is 64 hex chars.
pub fn static_label(content_hash_hex: &str) -> Option<Label> {
    let hash = blake3::Hash::from_hex(content_hash_hex).ok()?;
    Some(Label(format!("s-{}", base32_prefix(hash.as_bytes(), LABEL_HASH_CHARS))))
}

/// The label of a bundle URL in either spelling (protocol.md §1.4-1.5), or `None`.
pub fn parse(raw: &str) -> Option<Label> {
    split(raw).map(|(label, _)| label)
}

/// `(label, rest)` where `rest` is everything after the host: path, query and fragment, untouched.
///
/// Refuses userinfo, any port, a trailing dot, extra host labels, uppercase spellings and other schemes:
/// the authority must be exactly `<label>.localhost`.
pub fn split(raw: &str) -> Option<(Label, &str)> {
    let rest = raw
        .strip_prefix(HTTPS_PREFIX)
        .or_else(|| raw.strip_prefix(CUSTOM_PREFIX))?;
    let end = rest
        .find(|c: char| matches!(c, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let label = Label::parse(authority.strip_suffix(HOST_SUFFIX)?)?;
    Some((label, tail))
}

/// RFC 4648 base32 (lowercase, unpadded) of `bytes`, truncated to `chars` characters.
fn base32_prefix(bytes: &[u8], chars: usize) -> String {
    let mut out = String::with_capacity(chars);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32_ALPHABET[((buffer >> bits) & 31) as usize] as char);
            if out.len() == chars {
                return out;
            }
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 && out.len() < chars {
        out.push(BASE32_ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}
