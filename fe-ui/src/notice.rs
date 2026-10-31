//! Notice severity model (OD-1) + humanized `DbResult::Error` copy. Backs
//! `actions::mod::UiManager`'s toast/advisory/error surface. See
//! `fe-ui/src/actions/AGENTS.md` §notices for the queue/eviction rules this
//! type supports.

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

/// One queued user-facing message.
#[derive(Debug, Clone)]
pub struct Notice {
    pub id: u64,
    pub severity: Severity,
    pub text: String,
    pub spawned_at: f64,
}

/// Inputs `humanize_db_error` needs to name the acting role and verse in a
/// role-denial sentence.
pub struct DenialContext<'a> {
    pub role_label: &'a str,
    pub verse_name: Option<&'a str>,
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
/// failure, shown short. The caller (`db_results/query.rs::handle_error`)
/// keeps logging the untruncated raw string, so nothing is actually lost —
/// only kept out of the on-screen notice.
pub fn humanize_db_error(raw: &str, ctx: &DenialContext) -> String {
    if let Some(idx) = raw.find("permission denied") {
        let prefix = raw[..idx].trim_end_matches([':', ' ']);
        let verb = verb_for_prefix(prefix);
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
    fn severity_ord_ranks_error_above_advisory_above_info() {
        assert!(Severity::Error > Severity::Advisory);
        assert!(Severity::Advisory > Severity::Info);
    }
}
