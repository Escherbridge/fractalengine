//! Display-safe publisher text; see `fe-ui/src/model_library/AGENTS.md` §strings.

/// Strip control and bidi/invisible format characters, collapse whitespace runs (newlines included) to one space, trim, and clip to `max_chars` with an ellipsis.
pub fn display_text(input: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(input.len().min(max_chars.saturating_mul(4)));
    let mut count = 0usize;
    let mut pending_space = false;
    for character in input.chars() {
        if character.is_whitespace() {
            pending_space = count > 0;
            continue;
        }
        if character.is_control() || is_hidden_format(character) {
            continue;
        }
        let needed = if pending_space { 2 } else { 1 };
        if count + needed > max_chars {
            out.push('\u{2026}');
            return out;
        }
        if pending_space {
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        out.push(character);
        count += 1;
    }
    out
}

/// Bidirectional marks, embeddings, overrides and isolates, plus zero-width spacing and the BOM.
fn is_hidden_format(character: char) -> bool {
    matches!(
        character,
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::display_text;

    #[test]
    fn display_text_table() {
        let cases: &[(&str, usize, &str)] = &[
            ("Oak chair", 40, "Oak chair"),
            ("  two\nlines\r\n\there  ", 40, "two lines here"),
            ("bell\u{7}\u{1b}[31mred\u{7f}", 40, "bell[31mred"),
            ("invoice\u{202E}fdp.exe", 40, "invoicefdp.exe"),
            ("\u{2066}isolated\u{2069}\u{200F}\u{061C}", 40, "isolated"),
            ("zero\u{200B}width\u{FEFF}", 40, "zerowidth"),
            ("abcdef", 3, "abc\u{2026}"),
            ("abc def", 4, "abc\u{2026}"),
            ("abc def", 5, "abc d\u{2026}"),
            ("\u{e9}\u{e9}\u{e9}\u{e9}", 2, "\u{e9}\u{e9}\u{2026}"),
            ("exactly", 7, "exactly"),
            ("\n\n", 10, ""),
            ("", 5, ""),
        ];
        for (input, max_chars, expected) in cases {
            assert_eq!(display_text(input, *max_chars), *expected, "input {input:?} max {max_chars}");
        }
    }
}
