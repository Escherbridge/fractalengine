//! Feeds the local verse set to fe-api's opt-in dev-token writer
//! (`FE_API_DEV_TOKEN_OUT`) — see src/AGENTS.md §qa-dev-token.

use bevy::prelude::*;
use fe_ui::verse_manager::VerseManager;

/// Sender half of the writer's verse-id channel (present only when opted in).
#[derive(Resource)]
pub(crate) struct DevTokenVerseFeed(crossbeam::channel::Sender<Vec<String>>);

/// Register the feed system; call only when `FE_API_DEV_TOKEN_OUT` is set.
pub(crate) fn install(app: &mut App, verse_ids_tx: crossbeam::channel::Sender<Vec<String>>) {
    app.insert_resource(DevTokenVerseFeed(verse_ids_tx));
    app.add_systems(Update, feed_verse_ids_to_dev_token_writer);
}

/// Send the verse id list on the first frame and whenever the SET of verses
/// changes (create/join/delete), so the token file follows scenario-created
/// verses. Allocation-free on frames where nothing changed.
fn feed_verse_ids_to_dev_token_writer(
    verses: Res<VerseManager>,
    feed: Res<DevTokenVerseFeed>,
    mut sent: Local<Option<Vec<String>>>,
    mut writer_gone: Local<bool>,
) {
    if *writer_gone || (sent.is_some() && !verses.is_changed()) {
        return;
    }
    let current = verses.verses.iter().map(|verse| &verse.id);
    if sent.as_ref().is_some_and(|ids| ids.iter().eq(current)) {
        return;
    }
    let ids: Vec<String> = verses.verses.iter().map(|verse| verse.id.clone()).collect();
    if feed.0.send(ids.clone()).is_err() {
        warn!("dev-token writer has stopped; the token file will no longer follow verse changes");
        *writer_gone = true;
        return;
    }
    *sent = Some(ids);
}
