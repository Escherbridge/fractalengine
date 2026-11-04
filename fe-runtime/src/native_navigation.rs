//! Minimal local native-navigation reads; see src/AGENTS.md.

/// A rendered Petal's exact ancestry and displayed name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetalTarget {
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
    pub name: String,
}

impl PetalTarget {
    pub fn is_bounded(&self) -> bool {
        [&self.verse_id, &self.fractal_id, &self.petal_id]
            .into_iter()
            .all(|id| !id.is_empty() && id.len() <= 128 && !id.contains('\0'))
            && !self.name.is_empty()
            && self.name.len() <= 1024
            && self.name.encode_utf16().count() <= 256
            && !self.name.contains('\0')
    }
}

/// A successful DB-local Viewer read, without roles, grants, tokens or data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetalView {
    pub target: PetalTarget,
    pub source_fence: String,
}

impl PetalView {
    pub fn is_bounded(&self) -> bool {
        self.target.is_bounded()
            && self.source_fence.len() == 64
            && self.source_fence.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// All absent, changed, denied, malformed and timed-out local reads look alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    Target,
}
