//! Visible-text classification for publisher-supplied strings; see `src/AGENTS.md` §text.

/// Characters that render as nothing: bidi marks, embeddings, overrides and isolates, zero-width and joiner characters, invisible operators, blank fillers, variation selectors, tag characters and the BOM.
pub fn is_hidden_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{E0000}'..='\u{E007F}'
            | '\u{E0100}'..='\u{E01EF}'
    )
}

/// True when `text` has at least one character that is not whitespace, a control character or [`is_hidden_format`].
pub fn has_visible_text(text: &str) -> bool {
    text.chars()
        .any(|character| !character.is_whitespace() && !character.is_control() && !is_hidden_format(character))
}
