//! Window captures for agents (API bridge) and humans (F12 → file + toast) —
//! see src/AGENTS.md §screenshot and fe-api/AGENTS.md §screenshot.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::image::Image;
use bevy::input::ButtonInput;
use bevy::prelude::{
    App, Commands, Entity, KeyCode, On, PostUpdate, Query, Res, ResMut, Resource, Time, Update,
    With,
};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::AsyncComputeTaskPool;
use bevy::window::{PrimaryWindow, Window};
use fe_renderer::camera::CameraCaptureLock;
use fe_runtime::screenshot::{
    ScreenshotRequest, ScreenshotRequestReceiver, SCREENSHOT_SCOPE_DENIED,
};
use fe_ui::navigation_manager::NavigationManager;

use crate::vision_bridge::ViewScope;

/// Bevy side of the API→Bevy capture bridge.
#[derive(Resource)]
pub struct ScreenshotBridge(pub ScreenshotRequestReceiver);

/// Off-thread F12 save outcomes (saved path or error text) awaiting a toast.
#[derive(Resource)]
pub struct ScreenshotSaveResults {
    tx: crossbeam::channel::Sender<Result<PathBuf, String>>,
    rx: crossbeam::channel::Receiver<Result<PathBuf, String>>,
}

impl Default for ScreenshotSaveResults {
    fn default() -> Self {
        let (tx, rx) = crossbeam::channel::bounded(16);
        Self { tx, rx }
    }
}

/// Encode a captured frame as PNG. Alpha is dropped: with HDR it carries
/// brightness, not coverage (Bevy's own `save_to_disk` does the same).
pub fn encode_png(image: Image) -> Result<Vec<u8>, String> {
    let rgb = image
        .try_into_dynamic()
        .map_err(|error| format!("unsupported frame format: {error}"))?
        .to_rgb8();
    let mut png = Vec::new();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| format!("PNG encode failed: {error}"))?;
    Ok(png)
}

/// Capture `screenshot`'s target once and hand the PNG-encoded frame to
/// `on_encoded` on the compute pool — the frame loop never encodes. THE one
/// capture path: F12, the plain GET bridge and agent-vision captures
/// (window or render-to-texture) all go through it. Returns the
/// `Screenshot` entity so a torn-down caller can despawn a capture that
/// will never fire (src/AGENTS.md §vision).
pub(crate) fn capture_png(
    commands: &mut Commands,
    screenshot: Screenshot,
    on_encoded: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
) -> Entity {
    // Observers are `Fn`; the slot runs the one-shot continuation exactly once.
    let continuation = Arc::new(Mutex::new(Some(on_encoded)));
    commands
        .spawn(screenshot)
        .observe(move |captured: On<ScreenshotCaptured>| {
            let Some(on_encoded) = continuation.lock().ok().and_then(|mut slot| slot.take()) else {
                return;
            };
            let image = captured.image.clone();
            AsyncComputeTaskPool::get()
                .spawn(async move { on_encoded(encode_png(image)) })
                .detach();
        })
        .id()
}

/// Capture the primary window once (egui included); see [`capture_png`].
fn capture_primary_window_png(
    commands: &mut Commands,
    on_encoded: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
) {
    capture_png(commands, Screenshot::primary_window(), on_encoded);
}

/// Why the primary window cannot be captured right now, if it cannot. A
/// minimized window is 0x0: Bevy would mark its `Screenshot` `Capturing` and
/// never fire it (src/AGENTS.md §screenshot), so refuse up front.
pub fn capture_refusal(primary_window: Option<&Window>) -> Option<&'static str> {
    match primary_window {
        None => Some("no application window (closing?)"),
        Some(window) if window.physical_width() == 0 || window.physical_height() == 0 => {
            Some("the application window is minimized — restore it to capture")
        }
        Some(_) => None,
    }
}

/// Plain-GET requests that passed the arrival scope check, held for the
/// frame-time re-check in [`capture_pending_screenshots`].
#[derive(Resource, Default)]
pub struct PendingScreenshots(Vec<ScreenshotRequest>);

/// Register the plain-GET bridge: the arrival check in `Update`, the
/// frame-time re-check + capture in `PostUpdate` (src/AGENTS.md §screenshot).
pub fn install(app: &mut App, receiver: ScreenshotRequestReceiver) {
    app.insert_resource(ScreenshotBridge(receiver))
        .init_resource::<PendingScreenshots>()
        .add_systems(Update, drain_screenshot_requests)
        .add_systems(PostUpdate, capture_pending_screenshots);
}

/// Answer `request` with the uniform scope denial (agent_vision SEC-ADDENDUM).
fn deny(request: ScreenshotRequest) {
    request.reply(Err(SCREENSHOT_SCOPE_DENIED.to_string()));
}

/// Update (arrival check): drain the bridge, deny at once every caller whose
/// token scope does not cover the view (`ViewScope`: the active verse and
/// every petal still rendered), and hold the rest for this frame's
/// [`capture_pending_screenshots`]. While a posed agent-vision capture holds
/// the camera (`CameraCaptureLock`) the queue is left untouched, so a plain
/// GET never returns a half-posed frame. Abandoned requests are dropped.
pub fn drain_screenshot_requests(
    bridge: Res<ScreenshotBridge>,
    capture_lock: Option<Res<CameraCaptureLock>>,
    nav: Option<Res<NavigationManager>>,
    view: ViewScope,
    mut pending: ResMut<PendingScreenshots>,
) {
    if capture_lock.is_some() {
        return;
    }
    for request in bridge.0.try_iter() {
        if request.reply_tx.is_closed() {
            continue;
        }
        if view.covers(&request.caller_scope, nav.as_deref()) {
            pending.0.push(request);
        } else {
            deny(request);
        }
    }
}

/// PostUpdate (frame-time check, the vision path's rule): re-check every held
/// request's scope against the verse open NOW — the user may have switched
/// verse since the drain — then serve the covered ones with ONE capture of
/// the primary window, each replied to with its own correlation id. A lock
/// taken since the drain (a posed job started this frame) keeps them held.
pub fn capture_pending_screenshots(
    mut pending: ResMut<PendingScreenshots>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    capture_lock: Option<Res<CameraCaptureLock>>,
    nav: Option<Res<NavigationManager>>,
    view: ViewScope,
    mut commands: Commands,
) {
    if capture_lock.is_some() || pending.0.is_empty() {
        return;
    }
    // A request held across a lock may have been abandoned meanwhile (the
    // caller's 5 s timeout): drop it rather than capture for nobody.
    let (covered, uncovered): (Vec<ScreenshotRequest>, Vec<ScreenshotRequest>) =
        std::mem::take(&mut pending.0)
            .into_iter()
            .filter(|request| !request.reply_tx.is_closed())
            .partition(|request| view.covers(&request.caller_scope, nav.as_deref()));
    uncovered.into_iter().for_each(deny);
    if covered.is_empty() {
        return;
    }
    if let Some(reason) = capture_refusal(primary_window.single().ok()) {
        for request in covered {
            request.reply(Err(reason.to_string()));
        }
        return;
    }
    capture_primary_window_png(&mut commands, move |png| {
        for request in covered {
            request.reply(png.clone());
        }
    });
}

/// `data/screenshots` beside the database (the app data dir), absolute.
pub fn screenshots_dir() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("data")
        .join("screenshots")
}

/// `fractalengine-YYYYMMDD-HHMMSS-mmm.png` (UTC) for a Unix-epoch millisecond time.
pub fn screenshot_file_name(unix_millis: u64) -> String {
    let seconds = unix_millis / 1000;
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let second_of_day = seconds % 86_400;
    format!(
        "fractalengine-{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{:03}.png",
        second_of_day / 3600,
        (second_of_day / 60) % 60,
        second_of_day % 60,
        unix_millis % 1000
    )
}

/// Days since 1970-01-01 → (year, month, day) — Howard Hinnant's
/// `civil_from_days` (https://howardhinnant.github.io/date_algorithms.html).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Bevy system: F12 saves a timestamped PNG of the window to
/// `data/screenshots/` (local keyboard — no API gate) and reports via a toast.
pub fn capture_screenshot_on_f12(
    keys: Res<ButtonInput<KeyCode>>,
    results: Res<ScreenshotSaveResults>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::F12) {
        return;
    }
    let unix_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let path = screenshots_dir().join(screenshot_file_name(unix_millis));
    let tx = results.tx.clone();
    capture_primary_window_png(&mut commands, move |png| {
        let outcome = png.and_then(|png| {
            let dir = path.parent().map(PathBuf::from).unwrap_or_default();
            std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&path, png))
                .map(|()| path.clone())
                .map_err(|error| format!("could not write {}: {error}", path.display()))
        });
        if tx.try_send(outcome).is_err() {
            tracing::warn!("screenshot result dropped — toast queue full");
        }
    });
}

/// Bevy system: toast each finished F12 save (path on success, reason on failure).
pub fn surface_saved_screenshots(
    results: Res<ScreenshotSaveResults>,
    mut ui_mgr: ResMut<fe_ui::actions::UiManager>,
    time: Res<Time>,
) {
    while let Ok(outcome) = results.rx.try_recv() {
        let now = time.elapsed_secs_f64();
        match outcome {
            Ok(path) => {
                tracing::info!("Screenshot saved to {}", path.display());
                ui_mgr.show_toast(format!("Screenshot saved to {}", path.display()), now);
            }
            Err(error) => {
                tracing::warn!("Screenshot failed: {error}");
                ui_mgr.show_toast(format!("Screenshot failed: {error}"), now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    /// A captured BGRA frame (the common swap-chain format) round-trips to a
    /// PNG whose pixels are the frame's colours in RGB order.
    #[test]
    fn bgra_frame_encodes_to_a_png_with_rgb_pixels() {
        let (blue, green, red, alpha) = (10u8, 20u8, 200u8, 255u8);
        let frame = Image::new_fill(
            Extent3d {
                width: 3,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[blue, green, red, alpha],
            TextureFormat::Bgra8UnormSrgb,
            bevy::asset::RenderAssetUsages::default(),
        );
        let png = encode_png(frame).expect("encodes");
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("valid PNG")
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert!(decoded.pixels().all(|p| p.0 == [red, green, blue]));
    }

    use bevy::ecs::system::RunSystemOnce;
    use bevy::ecs::world::World;
    use fe_runtime::screenshot::{CapturePrimaryWindow, ScreenshotReply, ScreenshotRequestSender};

    /// A world with `verse` open (or nothing), a primary window of `size`,
    /// and the plain-GET bridge installed; returns the API end of the bridge.
    fn bridge_world(verse: Option<&str>, size: (u32, u32)) -> (World, ScreenshotRequestSender) {
        let mut world = World::new();
        let mut nav = NavigationManager::default();
        if let Some(verse) = verse {
            nav.navigate_to_verse(verse, "Verse");
        }
        world.insert_resource(nav);
        let (tx, rx) = fe_runtime::screenshot::screenshot_bridge();
        world.insert_resource(ScreenshotBridge(rx));
        world.init_resource::<PendingScreenshots>();
        let mut window = Window::default();
        window.resolution.set_physical_resolution(size.0, size.1);
        world.spawn((window, PrimaryWindow));
        (world, tx)
    }

    /// Queue one plain-GET request on behalf of `scope`.
    fn send(
        tx: &ScreenshotRequestSender,
        correlation_id: &str,
        scope: &str,
    ) -> tokio::sync::oneshot::Receiver<ScreenshotReply> {
        let (request, reply_rx) =
            ScreenshotRequest::new(correlation_id, scope, CapturePrimaryWindow);
        tx.try_send(request).expect("bridge has room");
        reply_rx
    }

    fn drain(world: &mut World) {
        world
            .run_system_once(drain_screenshot_requests)
            .expect("drain runs");
    }

    fn capture(world: &mut World) {
        world
            .run_system_once(capture_pending_screenshots)
            .expect("capture runs");
    }

    /// One frame: the Update drain, then the PostUpdate capture.
    fn frame(world: &mut World) {
        drain(world);
        capture(world);
    }

    fn captures(world: &mut World) -> usize {
        world.query::<&Screenshot>().iter(world).count()
    }

    /// SEC-ADDENDUM on the plain GET path: in one batch, a verse-B token gets
    /// the uniform denial at once while the verse-A token is still served by
    /// the one shared capture; with nothing open, everyone is denied.
    #[test]
    fn plain_captures_are_refused_unless_the_token_covers_the_active_verse() {
        for (open_verse, expect_capture) in [(Some("a"), true), (None, false)] {
            let (mut world, tx) = bridge_world(open_verse, (1280, 720));
            let mut foreign = send(&tx, "b", "VERSE#b");
            let mut covered = send(&tx, "a", "VERSE#a");
            frame(&mut world);
            let denied = foreign.try_recv().expect("verse-B answered at once");
            assert_eq!(denied.result, Err(SCREENSHOT_SCOPE_DENIED.to_string()));
            if expect_capture {
                assert_eq!(captures(&mut world), 1, "the covered caller is still served");
                assert!(covered.try_recv().is_err(), "awaiting the frame");
            } else {
                assert_eq!(captures(&mut world), 0, "nothing open: no one is covered");
                let reply = covered.try_recv().expect("answered at once");
                assert_eq!(reply.result, Err(SCREENSHOT_SCOPE_DENIED.to_string()));
            }
        }
    }

    /// The frame-time re-check: a request admitted at drain (verse A open) is
    /// refused, and no frame is taken, when the user switches to a verse the
    /// token does not cover before the capture is queued; staying put serves it.
    #[test]
    fn switching_to_an_uncovered_verse_between_drain_and_capture_refuses() {
        for (switch_to, expect_capture) in [(Some("b"), false), (None, true)] {
            let (mut world, tx) = bridge_world(Some("a"), (1280, 720));
            let mut reply = send(&tx, "c1", "VERSE#a");
            drain(&mut world);
            assert!(reply.try_recv().is_err(), "admitted at arrival, not yet answered");
            if let Some(verse) = switch_to {
                world
                    .resource_mut::<NavigationManager>()
                    .navigate_to_verse(verse, "B");
            }
            capture(&mut world);
            if expect_capture {
                assert_eq!(captures(&mut world), 1);
                assert!(reply.try_recv().is_err(), "awaiting the frame");
            } else {
                assert_eq!(captures(&mut world), 0, "no verse-B pixels for a verse-A token");
                let denied = reply.try_recv().expect("answered at once");
                assert_eq!(denied.correlation_id, "c1");
                assert_eq!(denied.result, Err(SCREENSHOT_SCOPE_DENIED.to_string()));
            }
            assert!(world.resource::<PendingScreenshots>().0.is_empty());
        }
    }

    fn verse(id: &str, petal: &str) -> fe_ui::verse_manager::VerseEntry {
        use fe_ui::verse_manager::{FractalEntry, PetalEntry, VerseEntry};
        VerseEntry {
            id: id.into(),
            name: id.into(),
            fractals: vec![FractalEntry {
                id: format!("f-{id}"),
                name: id.into(),
                petals: vec![PetalEntry {
                    id: petal.into(),
                    name: petal.into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// The plain GET path shares the view rule: just after the user opens
    /// verse a, verse b's map is still bound (terrain re-binds only when a's
    /// petal terrain loads), so a verse-a token is denied and no frame is
    /// taken; once only a's content is bound it is served.
    #[test]
    fn a_map_still_bound_from_an_uncovered_verse_refuses_plain_captures() {
        use fe_terrain::petal_binding::ActivePetalTerrain;
        let (mut world, tx) = bridge_world(Some("a"), (1280, 720));
        world.insert_resource(fe_ui::verse_manager::VerseManager::from_verses(vec![
            verse("a", "pa"),
            verse("b", "pb"),
        ]));
        world.insert_resource(ActivePetalTerrain {
            raw_source_fingerprint: None,
            assignment_fingerprint: None,
            input_scale_valid: false,
            petal_id: Some("pb".into()),
            config: Some(fe_terrain::config::TerrainConfig::default()),
            revision: 1,
        });
        let mut reply = send(&tx, "c1", "VERSE#a");
        frame(&mut world);
        assert_eq!(captures(&mut world), 0, "no frame showing verse b's map");
        let denied = reply.try_recv().expect("answered at once");
        assert_eq!(denied.result, Err(SCREENSHOT_SCOPE_DENIED.to_string()));

        world.resource_mut::<ActivePetalTerrain>().petal_id = Some("pa".into());
        let _served = send(&tx, "c2", "VERSE#a");
        frame(&mut world);
        assert_eq!(captures(&mut world), 1);
    }

    /// A request whose caller already hung up is dropped, never captured —
    /// at the drain, and when it was admitted but then held across a posed
    /// job's lock long enough for its caller to time out.
    #[test]
    fn an_abandoned_plain_request_takes_no_frame() {
        let (mut world, tx) = bridge_world(Some("v"), (1280, 720));
        drop(send(&tx, "gone", "VERSE#v"));
        frame(&mut world);
        assert_eq!(captures(&mut world), 0);

        let held = send(&tx, "held", "VERSE#v");
        drain(&mut world);
        world.insert_resource(CameraCaptureLock);
        capture(&mut world);
        drop(held);
        world.remove_resource::<CameraCaptureLock>();
        capture(&mut world);
        assert_eq!(captures(&mut world), 0, "no frame for a caller who left");
        assert!(world.resource::<PendingScreenshots>().0.is_empty());
    }

    /// A minimized (0x0) window is refused at once, with the request's own
    /// correlation id — never a `Screenshot` Bevy would leave `Capturing`
    /// forever while the agent waits out the 5 s timeout.
    #[test]
    fn a_minimized_window_refuses_captures_immediately() {
        let (mut world, tx) = bridge_world(Some("v"), (0, 0));
        let mut reply = send(&tx, "c1", "VERSE#v");
        frame(&mut world);
        assert_eq!(captures(&mut world), 0, "no capture queued for a 0x0 window");
        let reply = reply.try_recv().expect("answered in the same frame");
        assert_eq!(reply.correlation_id, "c1");
        assert!(
            reply
                .result
                .as_ref()
                .is_err_and(|e| e.contains("minimized")),
            "{:?}",
            reply.result
        );
    }

    #[test]
    fn a_visible_window_queues_one_capture_and_answers_later() {
        let (mut world, tx) = bridge_world(Some("v"), (1280, 720));
        let mut reply = send(&tx, "c1", "VERSE#v");
        frame(&mut world);
        assert_eq!(captures(&mut world), 1);
        assert!(reply.try_recv().is_err(), "the reply waits for the captured frame");
    }

    /// While a posed agent-vision capture holds the camera, a plain GET stays
    /// QUEUED (not refused, not captured): it is served unchanged the first
    /// frame after the lock lifts, so it never sees a half-posed frame,
    /// including a request drained just before a job took the lock.
    #[test]
    fn plain_get_requests_stay_queued_while_a_posed_job_holds_the_camera() {
        let (mut world, tx) = bridge_world(Some("v"), (1280, 720));
        world.insert_resource(CameraCaptureLock);
        let mut reply = send(&tx, "queued", "VERSE#v");
        frame(&mut world);
        assert_eq!(captures(&mut world), 0);
        assert!(reply.try_recv().is_err(), "no reply while locked");
        assert_eq!(tx.len(), 1, "the request is still queued, not drained");

        world.remove_resource::<CameraCaptureLock>();
        drain(&mut world);
        assert_eq!(tx.len(), 0, "drained once the lock lifted");
        // A posed job takes the lock between this drain and the capture.
        world.insert_resource(CameraCaptureLock);
        capture(&mut world);
        assert_eq!(captures(&mut world), 0, "held, never captured mid-pose");
        assert_eq!(world.resource::<PendingScreenshots>().0.len(), 1);

        world.remove_resource::<CameraCaptureLock>();
        frame(&mut world);
        assert_eq!(captures(&mut world), 1);
        assert!(reply.try_recv().is_err(), "awaiting the frame");
    }

    #[test]
    fn screenshot_file_names_are_utc_timestamps() {
        let cases = [
            (0, "fractalengine-19700101-000000-000.png"),
            // 2024-02-29T23:59:59.123Z — a leap day, end of day.
            (1_709_251_199_123, "fractalengine-20240229-235959-123.png"),
            // 2026-10-31T12:47:05.000Z
            (1_793_450_825_000, "fractalengine-20261031-124705-000.png"),
        ];
        for (unix_millis, expected) in cases {
            assert_eq!(screenshot_file_name(unix_millis), expected, "{unix_millis}");
        }
    }
}
