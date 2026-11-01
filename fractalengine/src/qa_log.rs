//! Opt-in tracing capture; see src/AGENTS.md §qa-debug.

use fe_runtime::qa_debug::QaLogRing;
use std::fmt::Write;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

pub struct QaLogLayer(pub QaLogRing);

#[derive(Default)]
struct Fields {
    message: String,
    verse_id: Option<String>,
}

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let name = field.name().to_ascii_lowercase();
        if ["token", "jwt", "authorization", "secret", "password"]
            .iter()
            .any(|part| name.contains(part))
        {
            let _ = write!(self.message, "{}=[REDACTED] ", field.name());
        } else {
            let _ = write!(self.message, "{}={value:?} ", field.name());
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "verse_id" {
            self.verse_id = Some(value.to_owned());
        } else if field.name() == "scope" {
            if let Some(verse) = value.strip_prefix("VERSE#") {
                self.verse_id = Some(verse.split("-FRACTAL#").next().unwrap_or(verse).to_owned());
            }
        }
        self.record_debug(field, &value);
    }
}

impl<S: Subscriber> Layer<S> for QaLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        self.0.push(
            fields.verse_id.as_deref(),
            event.metadata().level().as_str(),
            event.metadata().target(),
            &fields.message,
        );
    }
}
