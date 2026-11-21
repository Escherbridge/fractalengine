//! Human rendering and terminal sanitising (S4).

use std::iter::Peekable;
use std::str::Chars;

use serde_json::Value;

use crate::envelope::Envelope;

/// Longest single-line string `terminal_safe` returns, in characters.
pub const TERMINAL_SAFE_MAX_CHARS: usize = 512;
/// Most lines `terminal_safe_multiline` keeps.
pub const TERMINAL_SAFE_MAX_LINES: usize = 200;
/// Widest table cell, in characters.
const TABLE_CELL_MAX_CHARS: usize = 72;

/// Strips control characters and ANSI/OSC escape sequences from untrusted text, flattens whitespace and bounds length.
pub fn terminal_safe(text: &str) -> String {
    let mut output = String::new();
    let mut count = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let kept = match c {
            '\u{1b}' => {
                skip_escape(&mut chars);
                None
            }
            '\u{9b}' => {
                skip_csi(&mut chars);
                None
            }
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => {
                skip_string(&mut chars);
                None
            }
            '\n' | '\r' | '\t' => Some(' '),
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' => None,
            other if other.is_control() => None,
            other => Some(other),
        };
        if let Some(kept) = kept {
            if count == TERMINAL_SAFE_MAX_CHARS {
                output.pop();
                output.push('\u{2026}');
                return output;
            }
            output.push(kept);
            count += 1;
        }
    }
    output
}

/// Sanitises text line by line (newlines kept), bounding both line length and line count.
pub fn terminal_safe_multiline(text: &str) -> String {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalised.split('\n').take(TERMINAL_SAFE_MAX_LINES + 1).map(terminal_safe).collect();
    if lines.len() > TERMINAL_SAFE_MAX_LINES {
        lines.truncate(TERMINAL_SAFE_MAX_LINES);
        lines.push("\u{2026}".to_string());
    }
    lines.join("\n")
}

/// Consumes what follows an ESC: CSI, a string sequence (OSC/DCS/APC/PM/SOS), or a short escape.
fn skip_escape(chars: &mut Peekable<Chars<'_>>) {
    match chars.next() {
        Some('[') => skip_csi(chars),
        Some(']' | 'P' | '_' | '^' | 'X') => skip_string(chars),
        Some(intermediate) if ('\u{20}'..='\u{2f}').contains(&intermediate) => {
            while chars.next_if(|next| ('\u{20}'..='\u{2f}').contains(next)).is_some() {}
            chars.next();
        }
        _ => {}
    }
}

/// Consumes CSI parameters and the final byte.
fn skip_csi(chars: &mut Peekable<Chars<'_>>) {
    while chars.next_if(|next| ('\u{20}'..='\u{3f}').contains(next)).is_some() {}
    chars.next_if(|next| ('\u{40}'..='\u{7e}').contains(next));
}

/// Consumes a string sequence up to BEL, ST (ESC `\` or U+009C) or the end of the text.
fn skip_string(chars: &mut Peekable<Chars<'_>>) {
    while let Some(c) = chars.next() {
        match c {
            '\u{07}' | '\u{9c}' => return,
            '\u{1b}' => {
                chars.next_if_eq(&'\\');
                return;
            }
            _ => {}
        }
    }
}

/// One `label: value` line with the value sanitised.
pub fn render_line(label: &str, value: &str) -> String {
    format!("{label}: {}", terminal_safe(value))
}

/// Left-aligned table with sanitised, bounded cells.
pub fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let cell = |text: &str| {
        let safe = terminal_safe(text);
        if safe.chars().count() > TABLE_CELL_MAX_CHARS {
            let mut shortened: String = safe.chars().take(TABLE_CELL_MAX_CHARS - 1).collect();
            shortened.push('\u{2026}');
            shortened
        } else {
            safe
        }
    };
    let mut grid: Vec<Vec<String>> = vec![headers.iter().map(|header| header.to_string()).collect()];
    for row in rows {
        grid.push(row.iter().map(|text| cell(text.as_str())).collect::<Vec<String>>());
    }
    let columns = grid.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| grid.iter().map(|row| row.get(column).map_or(0, |text| text.chars().count())).max().unwrap_or(0))
        .collect();
    grid.iter()
        .map(|row| {
            let padded: Vec<String> = row.iter().enumerate().map(|(column, text)| format!("{text:<width$}", width = widths[column])).collect();
            padded.join("  ").trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders an envelope for human mode; every extension-supplied string passes through `terminal_safe`.
pub fn render_human(envelope: &Envelope) -> String {
    if !envelope.ok {
        let mut lines = vec![format!("error[{}]: {}", terminal_safe(&envelope.code), terminal_safe(&envelope.message))];
        if let Some(reason) = &envelope.refusal.reason {
            let by = envelope.refusal.package_id.as_deref().map(|package| format!(" by {}", terminal_safe(package))).unwrap_or_default();
            lines.push(format!("refused{by}: {}", terminal_safe(reason)));
        }
        if let Some(milliseconds) = envelope.retry_after_ms {
            lines.push(format!("retry after {milliseconds} ms"));
        }
        return lines.join("\n");
    }
    match &envelope.data {
        Some(data) => render_data(data),
        None if envelope.message.is_empty() => "ok".to_string(),
        None => terminal_safe(&envelope.message),
    }
}

/// Shape-driven rendering: `stale`, `help`, `packages` and `package` documents, else pretty JSON.
fn render_data(data: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut structured = false;
    if data.get("stale").and_then(Value::as_bool) == Some(true) {
        parts.push("(stale: instance unreachable; showing the cached catalog)".to_string());
    }
    if let Some(help) = data.get("help").and_then(Value::as_str) {
        parts.push(terminal_safe_multiline(help));
        structured = true;
    }
    if let Some(packages) = data.get("packages").and_then(Value::as_array) {
        parts.push(package_table(packages));
        structured = true;
    } else if let Some(package) = data.get("package").filter(|package| package.is_object()) {
        parts.push(command_table(package));
        structured = true;
    }
    if !structured {
        parts.push(match data {
            Value::String(text) => terminal_safe_multiline(text),
            other => terminal_safe_multiline(&serde_json::to_string_pretty(other).unwrap_or_default()),
        });
    }
    parts.join("\n")
}

fn text_of(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn count_of(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_array).map_or(0, Vec::len).to_string()
}

fn package_table(packages: &[Value]) -> String {
    if packages.is_empty() {
        return "no extension commands at this petal".to_string();
    }
    let rows: Vec<Vec<String>> = packages
        .iter()
        .map(|package| vec![text_of(package, "package_id"), text_of(package, "version"), count_of(package, "commands"), count_of(package, "streams")])
        .collect();
    render_table(&["PACKAGE", "VERSION", "COMMANDS", "STREAMS"], &rows)
}

fn command_table(package: &Value) -> String {
    let heading = format!("{} {}", terminal_safe(&text_of(package, "package_id")), terminal_safe(&text_of(package, "version")));
    let commands = package.get("commands").and_then(Value::as_array).cloned().unwrap_or_default();
    if commands.is_empty() {
        return format!("{}\nno commands", heading.trim_end());
    }
    let rows: Vec<Vec<String>> = commands
        .iter()
        .map(|command| {
            let callable = if command.get("callable").and_then(Value::as_bool) == Some(true) { "yes" } else { "no" };
            vec![text_of(command, "id"), text_of(command, "role_floor"), callable.to_string(), text_of(command, "description")]
        })
        .collect();
    format!("{}\n{}", heading.trim_end(), render_table(&["COMMAND", "ROLE", "CALLABLE", "DESCRIPTION"], &rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_safe_table() {
        let long = "x".repeat(TERMINAL_SAFE_MAX_CHARS + 50);
        let cases: Vec<(String, String)> = vec![
            ("plain text".into(), "plain text".into()),
            ("a\u{1b}[31mred\u{1b}[0m".into(), "ared".into()),
            ("\u{1b}]0;title\u{07}after".into(), "after".into()),
            ("\u{1b}]8;;http://evil/\u{1b}\\click\u{1b}]8;;\u{1b}\\!".into(), "click!".into()),
            ("one\ntwo\r\nthree\tfour".into(), "one two  three four".into()),
            ("bell\u{07}nul\u{0}del\u{7f}".into(), "bellnuldel".into()),
            ("c1\u{9b}31mx".into(), "c1x".into()),
            ("a\u{202E}b\u{2066}c".into(), "abc".into()),
            // An extension description cannot forge a help line.
            ("Spawn\nrole floor: none; callable: yes".into(), "Spawn role floor: none; callable: yes".into()),
            ("\u{1b}".into(), "".into()),
        ];
        for (input, expected) in cases {
            assert_eq!(terminal_safe(&input), expected, "{input:?}");
        }
        let bounded = terminal_safe(&long);
        assert_eq!(bounded.chars().count(), TERMINAL_SAFE_MAX_CHARS);
        assert!(bounded.ends_with('\u{2026}'));
    }
}
