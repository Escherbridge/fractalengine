use std::time::{Duration, Instant};

const DEBOUNCE_MS: u64 = 300;

/// Bevy resource for the search query state.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub text: String,
    pub tag_filters: Vec<String>,
    /// Set to `Some(Instant)` when text changes; cleared when search fires.
    pub pending_since: Option<Instant>,
}

impl SearchQuery {
    /// Called whenever the text input changes.
    pub fn on_text_changed(&mut self, new_text: &str) {
        if new_text.is_empty() {
            self.text.clear();
            self.pending_since = None;
            return;
        }
        self.text = new_text.to_string();
        self.pending_since = Some(Instant::now());
    }

    /// Returns `true` if the debounce timer has elapsed and a search should fire.
    pub fn is_debounce_ready(&self) -> bool {
        self.pending_since
            .map(|t| t.elapsed() >= Duration::from_millis(DEBOUNCE_MS))
            .unwrap_or(false)
    }

    /// Toggle a tag filter: add if not present, remove if present.
    pub fn toggle_tag_filter(&mut self, tag: String) {
        if let Some(pos) = self.tag_filters.iter().position(|t| t == &tag) {
            self.tag_filters.remove(pos);
        } else {
            self.tag_filters.push(tag);
        }
    }

    /// Clear the pending timer after a search has been dispatched.
    pub fn mark_dispatched(&mut self) {
        self.pending_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_query_resource_default_is_empty() {
        let q = SearchQuery::default();
        assert!(q.text.is_empty());
        assert!(q.tag_filters.is_empty());
    }

    #[test]
    fn debounce_timer_resets_on_new_input() {
        let mut q = SearchQuery::default();
        q.on_text_changed("abc");
        let t1 = q.pending_since.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        q.on_text_changed("abcd");
        assert!(q.pending_since.unwrap() > t1);
    }

    #[test]
    fn debounce_not_ready_before_300ms() {
        let mut q = SearchQuery::default();
        q.on_text_changed("x");
        assert!(!q.is_debounce_ready());
    }

    #[test]
    fn filter_chips_compose_and() {
        let q = SearchQuery { tag_filters: vec!["foo".into(), "bar".into()], ..Default::default() };
        assert_eq!(q.tag_filters.len(), 2);
    }

    #[test]
    fn empty_query_does_not_set_pending() {
        let mut q = SearchQuery::default();
        q.on_text_changed("");
        assert!(q.pending_since.is_none());
    }

    #[test]
    fn tag_filter_toggle_adds_and_removes() {
        let mut q = SearchQuery::default();
        q.toggle_tag_filter("foo".into());
        assert!(q.tag_filters.contains(&"foo".to_string()));
        q.toggle_tag_filter("foo".into());
        assert!(!q.tag_filters.contains(&"foo".to_string()));
    }
}
