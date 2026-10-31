//! Window captures for agents (API bridge) and humans (F12 → file + toast) —
//! see src/AGENTS.md §screenshot and fe-api/AGENTS.md §screenshot.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::image::Image;
use bevy::input::ButtonInput;
use bevy::prelude::{Commands, KeyCode, On, Res, ResMut, Resource, Time};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::AsyncComputeTaskPool;
use fe_runtime::screenshot::{ScreenshotRequest, ScreenshotRequestReceiver};

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

/// Bevy system: serve every pending API capture request with ONE capture of
/// the primary window (egui included); encoding runs on the compute pool so
/// the frame loop never stalls on a PNG encode.
pub fn drain_screenshot_requests(bridge: Res<ScreenshotBridge>, mut commands: Commands) {
    let requests: Vec<ScreenshotRequest> = bridge.0.try_iter().collect();
    if requests.is_empty() {
        return;
    }
    let waiters = Arc::new(Mutex::new(Some(requests)));
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>| {
            let Some(requests) = waiters.lock().ok().and_then(|mut slot| slot.take()) else {
                return;
            };
            let image = captured.image.clone();
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let png = encode_png(image);
                    for request in requests {
                        request.reply(png.clone());
                    }
                })
                .detach();
        },
    );
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
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>| {
            let image = captured.image.clone();
            let (path, tx) = (path.clone(), tx.clone());
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let outcome = encode_png(image).and_then(|png| {
                        let dir = path.parent().map(PathBuf::from).unwrap_or_default();
                        std::fs::create_dir_all(&dir)
                            .and_then(|()| std::fs::write(&path, png))
                            .map(|()| path.clone())
                            .map_err(|error| format!("could not write {}: {error}", path.display()))
                    });
                    if tx.try_send(outcome).is_err() {
                        tracing::warn!("screenshot result dropped — toast queue full");
                    }
                })
                .detach();
        },
    );
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
