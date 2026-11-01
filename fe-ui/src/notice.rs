//! Notice severity model (OD-1) + humanized `DbResult::Error` copy. Backs
//! `actions::mod::UiManager`'s toast/advisory/error surface. See
//! `fe-ui/src/actions/AGENTS.md` §notices for the queue/eviction rules this
//! type supports.

use fe_database::RoleLevel;

use crate::navigation_manager::NavigationManager;
use crate::plugin::LocalUserRole;

/// How urgently a [`Notice`] competes for the single on-screen slot, and how
/// long it survives unattended before `UiManager::active_notice` stops
/// returning it (OD-1 defaults). Declared Info < Advisory < Error so the
/// derived `Ord` doubles as the display-priority order `UiManager` sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Advisory,
    Error,
}

impl Severity {
    /// Visible lifetime in seconds before a non-held notice expires (OD-1).
    pub fn duration_secs(self) -> f64 {
        match self {
            Severity::Info => 2.5,
            Severity::Advisory => 4.0,
            Severity::Error => 7.0,
        }
    }
}

/// The one clock every notice is stamped and rendered on. bevy_egui feeds
/// `RawInput::time` from `Time<Real>`, so a Bevy system that stamps with the
/// default `Time` (= `Time<Virtual>`, which clamps each frame to 250 ms and
/// lags after any hitch) makes its notices look older than they are — a
/// lag above 7 s hides every Error. See `actions/AGENTS.md` §notices.
pub type NoticeClock = bevy::time::Time<bevy::time::Real>;

/// One queued user-facing message.
#[derive(Debug, Clone)]
pub struct Notice {
    pub id: u64,
    pub severity: Severity,
    pub text: String,
    pub spawned_at: f64,
    /// The raw text behind a humanized `text` (a `DbResult::Error` string),
    /// kept for the status-bar history's detail line and Copy.
    pub detail: Option<String>,
}

/// Inputs `humanize_db_error` needs to name the acting role and verse in a
/// role-denial sentence.
pub struct DenialContext<'a> {
    pub role_label: &'a str,
    pub verse_name: Option<&'a str>,
}

impl<'a> DenialContext<'a> {
    /// The one way GUI code builds a denial context: the live role, labeled
    /// by [`role_label`], plus the verse the denial happened in.
    pub fn new(local_role: &LocalUserRole, verse_name: Option<&'a str>) -> Self {
        Self {
            role_label: role_label(local_role.role),
            verse_name,
        }
    }
}

/// What [`role_label`] shows while the local role is still unresolved
/// (R2 X-03: show "—", never guess a role).
pub const UNRESOLVED_ROLE_LABEL: &str = "\u{2014}";

/// User-facing role word for every role-naming surface — denial notices
/// (`humanize_db_error`) and disabled-control hovers
/// (`panels::widgets::role_denial_reason`) — so they never disagree.
/// `RoleLevel::None` (resolved: no grant in this verse) reads "Visitor";
/// an unresolved role reads [`UNRESOLVED_ROLE_LABEL`].
pub fn role_label(role: Option<RoleLevel>) -> &'static str {
    match role {
        Some(RoleLevel::Owner) => "Owner",
        Some(RoleLevel::Manager) => "Manager",
        Some(RoleLevel::Editor) => "Editor",
        Some(RoleLevel::Viewer) => "Viewer",
        Some(RoleLevel::None) => "Visitor",
        None => UNRESOLVED_ROLE_LABEL,
    }
}

/// The active verse's display name, or `None` on the verse browser — the
/// `verse_name` half of a [`DenialContext`].
pub fn active_verse_name(nav: &NavigationManager) -> Option<&str> {
    nav.active_verse_id
        .as_deref()
        .map(|_| nav.active_verse_name.as_str())
}

/// Longest raw-error fragment folded into the "Something went wrong" copy
/// for an error `humanize_db_error` does not recognize as a role denial.
const UNKNOWN_ERROR_SNIPPET_LEN: usize = 80;

/// Maps a known failing-command prefix — the text before "permission
/// denied" in a `DbResult::Error`, e.g. "Set petal terrain failed" (see
/// `fe-database/src/rbac.rs`'s `"permission denied: {reason}"` wrapper) — to
/// the plain-language verb phrase used in the humanized denial sentence.
/// Two prefixes get hand-written phrasing (M-03/D-05's user-facing copy);
/// every other known "<Verb> failed" prefix derives one mechanically so a
/// future `DbCommand` never falls back to the unknown-error branch just
/// because nobody wrote it a dedicated phrase.
fn verb_for_prefix(prefix: &str) -> String {
    match prefix {
        "Set petal terrain failed" => "change the map".to_string(),
        "Join verse failed" => "join this verse".to_string(),
        _ => match prefix.strip_suffix(" failed") {
            Some(base) if !base.is_empty() => base.to_lowercase(),
            _ => "do that".to_string(),
        },
    }
}

/// Turns a raw `DbResult::Error` string into plain-language copy for a
/// `Notice`. A `"<prefix>: permission denied: <reason>"` shape becomes a
/// role-denial sentence naming the verb; anything else is an unrecognized
/// failure, shown short. `UiManager::show_db_error` keeps the untruncated
/// raw string as the notice's `detail`, so the history never loses it.
pub fn humanize_db_error(raw: &str, ctx: &DenialContext) -> String {
    if let Some(idx) = raw.find("permission denied") {
        let prefix = raw[..idx].trim_end_matches([':', ' ']);
        let verb = verb_for_prefix(prefix);
        // An unresolved role cannot be named, so the sentence states only
        // what is needed instead of "You're a —".
        if ctx.role_label == UNRESOLVED_ROLE_LABEL {
            return match ctx.verse_name {
                Some(verse) => format!("Editor role needed in {verse} to {verb}"),
                None => format!("Editor role needed to {verb}"),
            };
        }
        match ctx.verse_name {
            Some(verse) => format!(
                "You're a {role} in {verse} — Editor role needed to {verb}",
                role = ctx.role_label,
                verse = verse,
            ),
            None => format!(
                "You're a {role} — Editor role needed to {verb}",
                role = ctx.role_label,
            ),
        }
    } else {
        let short = if raw.chars().count() > UNKNOWN_ERROR_SNIPPET_LEN {
            let truncated: String = raw.chars().take(UNKNOWN_ERROR_SNIPPET_LEN).collect();
            format!("{truncated}…")
        } else {
            raw.to_string()
        };
        format!("Something went wrong: {short}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_db_error_maps_denials_to_role_copy() {
        struct Case<'a> {
            raw: &'a str,
            role_label: &'static str,
            verse_name: Option<&'static str>,
            expected: String,
        }
        let long_raw = "x".repeat(100);
        let expected_truncated = format!(
            "Something went wrong: {}…",
            "x".repeat(UNKNOWN_ERROR_SNIPPET_LEN)
        );
        let cases = vec![
            Case {
                raw: "Set petal terrain failed: permission denied: not an editor",
                role_label: "Viewer",
                verse_name: Some("Acme"),
                expected: "You're a Viewer in Acme — Editor role needed to change the map"
                    .to_string(),
            },
            Case {
                raw: "Join verse failed: permission denied: invite expired",
                role_label: "Viewer",
                verse_name: None,
                expected: "You're a Viewer — Editor role needed to join this verse".to_string(),
            },
            Case {
                raw: "Delete node failed: permission denied: not an editor",
                role_label: "Viewer",
                verse_name: Some("Acme"),
                expected: "You're a Viewer in Acme — Editor role needed to delete node"
                    .to_string(),
            },
            // Unresolved role (X-03): states the need, never "You're a —".
            Case {
                raw: "Set petal terrain failed: permission denied: not an editor",
                role_label: UNRESOLVED_ROLE_LABEL,
                verse_name: Some("Acme"),
                expected: "Editor role needed in Acme to change the map".to_string(),
            },
            Case {
                raw: "Database connection lost",
                role_label: "Viewer",
                verse_name: None,
                expected: "Something went wrong: Database connection lost".to_string(),
            },
            Case {
                raw: &long_raw,
                role_label: "Viewer",
                verse_name: None,
                expected: expected_truncated.clone(),
            },
        ];
        for case in &cases {
            let ctx = DenialContext {
                role_label: case.role_label,
                verse_name: case.verse_name,
            };
            assert_eq!(
                humanize_db_error(case.raw, &ctx),
                case.expected,
                "raw={:?}",
                case.raw
            );
        }
    }

    #[test]
    fn role_label_names_every_role_and_never_guesses_an_unresolved_one() {
        let cases = [
            (Some(RoleLevel::Owner), "Owner"),
            (Some(RoleLevel::Manager), "Manager"),
            (Some(RoleLevel::Editor), "Editor"),
            (Some(RoleLevel::Viewer), "Viewer"),
            (Some(RoleLevel::None), "Visitor"),
            (None, UNRESOLVED_ROLE_LABEL),
        ];
        for (role, expected) in cases {
            assert_eq!(role_label(role), expected, "role={role:?}");
        }
    }

    #[test]
    fn severity_ord_ranks_error_above_advisory_above_info() {
        assert!(Severity::Error > Severity::Advisory);
        assert!(Severity::Advisory > Severity::Info);
    }
}
