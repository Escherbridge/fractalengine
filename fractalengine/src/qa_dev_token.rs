//! Feeds the local verse set to fe-api's opt-in dev-token writer
//! (`FE_API_DEV_TOKEN_OUT`) — see src/AGENTS.md §qa-dev-token.

use bevy::prelude::*;
use fe_database::RoleLevel;
use fe_ui::plugin::LocalUserRole;
use fe_ui::verse_manager::VerseManager;

/// Sender half of the writer's verse-id channel (present only when opted in).
#[derive(Resource)]
pub(crate) struct DevTokenVerseFeed(crossbeam::channel::Sender<Vec<String>>);

/// Register the feed system; call only when `FE_API_DEV_TOKEN_OUT` is set.
pub(crate) fn install(app: &mut App, verse_ids_tx: crossbeam::channel::Sender<Vec<String>>) {
    app.insert_resource(DevTokenVerseFeed(verse_ids_tx));
    app.add_systems(Update, feed_verse_ids_to_dev_token_writer);
}

/// Send the verse id list on the first frame, whenever the SET of verses
/// changes (create/join/delete), and whenever the resolved local role changes
/// (a joined verse's role statement can land after the verse does), so the
/// token file follows scenario-created verses and their settled roles.
/// Allocation-free on frames where nothing changed.
fn feed_verse_ids_to_dev_token_writer(
    verses: Res<VerseManager>,
    role: Res<LocalUserRole>,
    feed: Res<DevTokenVerseFeed>,
    mut sent: Local<Option<Vec<String>>>,
    mut sent_role: Local<Option<RoleLevel>>,
    mut writer_gone: Local<bool>,
) {
    let role_changed = sent.is_some() && *sent_role != role.role;
    if *writer_gone || (sent.is_some() && !verses.is_changed() && !role_changed) {
        return;
    }
    let current = verses.verses.iter().map(|verse| &verse.id);
    if !role_changed && sent.as_ref().is_some_and(|ids| ids.iter().eq(current)) {
        return;
    }
    let ids: Vec<String> = verses.verses.iter().map(|verse| verse.id.clone()).collect();
    if feed.0.send(ids.clone()).is_err() {
        warn!("dev-token writer has stopped; the token file will no longer follow verse changes");
        *writer_gone = true;
        return;
    }
    *sent = Some(ids);
    *sent_role = role.role;
}
