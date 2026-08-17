//! SPEC-8 §3 local migration-mode flag surface: five mutually exclusive modes,
//! read from the local process environment only. See
//! `fe-database/src/migration/AGENTS.md` §flag-isolation for why an env var was
//! chosen over a cargo feature or a database row.

use std::env::VarError;
use std::fmt;

/// The only env var [`MigrationFlags::from_env`] reads (§2.2: local process only).
pub const MIGRATION_MODE_ENV_VAR: &str = "FE_MIGRATION_MODE";

/// One of the five mutually exclusive SPEC-8 §3 migration modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MigrationMode {
    /// Execute the current path only; no candidate is ever created. Default (§3.1).
    #[default]
    LegacyOnly,
    /// Execute the current path through the single future ingress; still no candidate.
    SeamReady,
    /// Execute the current path once, then derive and retain a correlated candidate.
    DualEmitShadow,
    /// Execute the current path once, then replay the retained candidate set.
    ShadowRebuild,
    /// The approved canonical local projection is authoritative. Owner-gated (§3.4);
    /// [`MigrationFlags::from_env`] always refuses this token in the current build.
    CanonicalAuthoritativeLocal,
}

impl MigrationMode {
    /// The exact lower_snake_case `FE_MIGRATION_MODE` token this mode parses from
    /// and round-trips to for diagnostics (§2.2, §3 table).
    pub fn as_token(self) -> &'static str {
        match self {
            Self::LegacyOnly => "legacy_only",
            Self::SeamReady => "seam_ready",
            Self::DualEmitShadow => "dual_emit_shadow",
            Self::ShadowRebuild => "shadow_rebuild",
            Self::CanonicalAuthoritativeLocal => "canonical_authoritative_local",
        }
    }
}

impl fmt::Display for MigrationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_token())
    }
}

/// Typed failure from [`MigrationFlags::from_env`] — never a silent fallback (§2.2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationFlagsError {
    /// `FE_MIGRATION_MODE` was set to a token that isn't one of the five modes.
    #[error("FE_MIGRATION_MODE={0:?} is not a recognized migration mode")]
    UnknownMode(String),
    /// `FE_MIGRATION_MODE=canonical_authoritative_local` — owner-gated, refused here.
    #[error("canonical_authoritative_local is owner-gated and unavailable in this build")]
    CanonicalAuthoritativeLocalUnavailable,
    /// `FE_MIGRATION_MODE` was present but not valid UTF-8.
    #[error("FE_MIGRATION_MODE is not valid UTF-8")]
    NotUnicode,
}

/// Local process-only migration flag state (§2.2): never remotely activated by a
/// peer, relay, WebSocket client, or replicated row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationFlags {
    mode: MigrationMode,
    defaulted: bool,
}

impl MigrationFlags {
    /// Read `FE_MIGRATION_MODE` from the local process environment only — never a
    /// config row, replicated value, or peer message (§2.2). Absence defaults to
    /// `LegacyOnly`; an unrecognized token or `canonical_authoritative_local` is a
    /// typed error, never a silent fallback.
    pub fn from_env() -> Result<Self, MigrationFlagsError> {
        match std::env::var(MIGRATION_MODE_ENV_VAR) {
            Ok(raw) => match raw.as_str() {
                "legacy_only" => Ok(Self {
                    mode: MigrationMode::LegacyOnly,
                    defaulted: false,
                }),
                "seam_ready" => Ok(Self {
                    mode: MigrationMode::SeamReady,
                    defaulted: false,
                }),
                "dual_emit_shadow" => Ok(Self {
                    mode: MigrationMode::DualEmitShadow,
                    defaulted: false,
                }),
                "shadow_rebuild" => Ok(Self {
                    mode: MigrationMode::ShadowRebuild,
                    defaulted: false,
                }),
                "canonical_authoritative_local" => {
                    Err(MigrationFlagsError::CanonicalAuthoritativeLocalUnavailable)
                }
                other => Err(MigrationFlagsError::UnknownMode(other.to_string())),
            },
            Err(VarError::NotPresent) => Ok(Self {
                mode: MigrationMode::default(),
                defaulted: true,
            }),
            Err(VarError::NotUnicode(_)) => Err(MigrationFlagsError::NotUnicode),
        }
    }

    /// The resolved migration mode.
    pub fn mode(&self) -> MigrationMode {
        self.mode
    }

    /// True when [`Self::mode`] came from the `LegacyOnly` default rather than an
    /// explicit `FE_MIGRATION_MODE` value.
    pub fn defaulted(&self) -> bool {
        self.defaulted
    }

    /// One-line diagnostic a process can log to state its mode (§2.2).
    pub fn diagnostic_summary(&self) -> String {
        if self.defaulted {
            format!(
                "migration_mode={} (source=default, {MIGRATION_MODE_ENV_VAR} unset)",
                self.mode.as_token()
            )
        } else {
            format!(
                "migration_mode={} (source={MIGRATION_MODE_ENV_VAR})",
                self.mode.as_token()
            )
        }
    }
}
