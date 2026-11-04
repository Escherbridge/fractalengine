//! Completed viewport-chrome geometry for shared pointer admission; see scene-pointer-design.md.

use bevy::prelude::{Entity, SystemSet, UVec2};
use bevy_egui::egui;

const CHROME_CAPACITY: usize = 4;

/// Camera input follows the UI's current window/navigation geometry observation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScenePointerContextSet;

/// Geometry ownership only; NavigationManager remains the navigation authority.
#[derive(Clone, Copy, PartialEq)]
pub struct ScenePointerStamp {
    window: Entity,
    physical_size: UVec2,
    native_scale: f32,
    navigation_revision: u64,
}

impl ScenePointerStamp {
    pub fn new(window: Entity, physical_size: UVec2, native_scale: f32, navigation_revision: u64) -> Self {
        Self { window, physical_size, native_scale, navigation_revision }
    }
    fn valid(self) -> bool {
        self.physical_size.x > 0 && self.physical_size.y > 0
            && self.native_scale.is_finite() && self.native_scale > 0.0
    }
}

#[derive(Clone, Copy)]
struct ChromeSnapshot {
    stamp: ScenePointerStamp,
    viewport: egui::ViewportId,
    screen: egui::Rect,
    scene: egui::Rect,
    pixels_per_point: f32,
    rects: [egui::Rect; CHROME_CAPACITY],
    count: usize,
    valid: bool,
}

impl ChromeSnapshot {
    fn contains(self, point: egui::Pos2) -> bool {
        self.rects[..self.count].iter().any(|rect| rect.contains(point))
    }
    fn current(self, ctx: &egui::Context, expected: Option<ScenePointerStamp>) -> bool {
        self.valid && expected == Some(self.stamp) && self.stamp.valid()
            && finite(self.screen) && self.screen.is_positive()
            && self.viewport == ctx.viewport_id() && self.screen == ctx.viewport_rect()
            && self.pixels_per_point.is_finite() && self.pixels_per_point > 0.0
            && self.pixels_per_point == ctx.pixels_per_point()
            && ((self.screen.width() * self.pixels_per_point) - self.stamp.physical_size.x as f32).abs() <= 1.0
            && ((self.screen.height() * self.pixels_per_point) - self.stamp.physical_size.y as f32).abs() <= 1.0
    }
}

/// Fixed-size publication; incomplete passes never become usable empty snapshots.
#[derive(Clone, Copy, Default)]
pub struct ScenePointerChrome {
    expected: Option<ScenePointerStamp>,
    completed: Option<ChromeSnapshot>,
    building: Option<ChromeSnapshot>,
}

fn key() -> egui::Id { egui::Id::new("scene-pointer-painted-chrome") }

fn finite(rect: egui::Rect) -> bool {
    [rect.min.x, rect.min.y, rect.max.x, rect.max.y].into_iter().all(f32::is_finite)
}

impl ScenePointerChrome {
    fn store(ctx: &egui::Context, state: Self) {
        ctx.data_mut(|data| *data.get_temp_mut_or_default::<Self>(key()) = state);
    }

    pub fn observe(ctx: &egui::Context, stamp: ScenePointerStamp) {
        let mut state = ctx.data(|data| data.get_temp::<Self>(key())).unwrap_or_default();
        let expected = stamp.valid().then_some(stamp);
        if state.expected != expected || state.building.is_some()
            || state.completed.is_some_and(|snapshot| !snapshot.current(ctx, expected))
        {
            state.completed = None;
            state.building = None;
        }
        state.expected = expected;
        Self::store(ctx, state);
    }

    pub fn invalidate(ctx: &egui::Context) {
        Self::store(ctx, Self::default());
    }

    pub fn begin_pass(ctx: &egui::Context, navigation_revision: u64) {
        let viewport = ctx.viewport_id();
        let screen = ctx.viewport_rect();
        let pixels_per_point = ctx.pixels_per_point();
        ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<Self>(key());
            state.building = state.expected.map(|stamp| ChromeSnapshot {
                stamp, viewport, screen, scene: egui::Rect::NOTHING,
                pixels_per_point, rects: [egui::Rect::NOTHING; CHROME_CAPACITY],
                count: 0, valid: stamp.navigation_revision == navigation_revision,
            });
        });
    }

    /// Register the actual response rect intersected with its painted clip.
    pub fn exclude(ctx: &egui::Context, rect: egui::Rect) {
        ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<Self>(key());
            if let Some(snapshot) = state.building.as_mut() {
                if !finite(rect) || snapshot.count == CHROME_CAPACITY { snapshot.valid = false; }
                else if rect.is_positive() {
                    snapshot.rects[snapshot.count] = rect;
                    snapshot.count += 1;
                }
            }
        });
    }

    pub fn finish_pass(ctx: &egui::Context, scene: egui::Rect, navigation_revision: u64) {
        let mut state = ctx.data(|data| data.get_temp::<Self>(key())).unwrap_or_default();
        if let Some(expected) = state.expected.as_mut() { expected.navigation_revision = navigation_revision; }
        let expected = state.expected;
        let building = state.building.take();
        state.completed = building.and_then(|mut snapshot| {
            snapshot.scene = scene;
            (snapshot.current(ctx, expected) && snapshot.count > 0
                && finite(scene) && scene.is_positive()).then_some(snapshot)
        });
        Self::store(ctx, state);
    }

    pub fn accepts(ctx: &egui::Context, scene: egui::Rect, pixels_per_point: f32, point: egui::Pos2) -> bool {
        let Some(mut state) = ctx.data(|data| data.get_temp::<Self>(key())) else { return false; };
        let Some(snapshot) = state.completed else { return false; };
        if !snapshot.current(ctx, state.expected) || snapshot.scene != scene
            || snapshot.pixels_per_point != pixels_per_point
        {
            state.completed = None;
            Self::store(ctx, state);
            return false;
        }
        !snapshot.contains(point)
    }

    pub fn matches_window(ctx: &egui::Context, window: Entity, size: UVec2, native_scale: f32) -> bool {
        let matches = ctx.data(|data| data.get_temp::<Self>(key())).and_then(|state| state.expected)
            .is_some_and(|expected| expected.window == window && expected.physical_size == size
                && expected.native_scale == native_scale);
        if !matches { Self::invalidate(ctx); }
        matches
    }

    /// Current UI-pass routing for the viewport's own context-menu producer.
    pub fn blocks_current_ui(ctx: &egui::Context, point: egui::Pos2) -> bool {
        let state = ctx.data(|data| data.get_temp::<Self>(key()));
        state.is_none_or(|state| {
            state.completed.is_none_or(|snapshot| !snapshot.current(ctx, state.expected)
                || !snapshot.scene.contains(point) || snapshot.contains(point))
                || state.building.is_some_and(|snapshot|
                    !snapshot.current(ctx, state.expected) || snapshot.contains(point))
        })
    }
}

#[cfg(test)]
pub(crate) fn assert_completed_chrome_journey() {
    use crate::camera::CameraPointerRegion;
    use bevy::prelude::Vec2;
    for scale in [1.0, 1.25, 2.0] {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(scale);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0));
        let stamp = ScenePointerStamp::new(Entity::from_bits(1),
            UVec2::new((1024.0 * scale) as u32, (768.0 * scale) as u32), 1.0, 7);
        let mut header = egui::Rect::NOTHING;
        let mut hint = egui::Rect::NOTHING;
        let mut scene = egui::Rect::NOTHING;
        for _ in 0..2 {
            let _output = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
                ScenePointerChrome::observe(ctx, stamp);
                ScenePointerChrome::begin_pass(ctx, 7);
                let panel = egui::CentralPanel::default().show(ctx, |ui| {
                    header = ui.horizontal(|ui| { let _ = ui.button("Back"); let _ = ui.button("Verse"); }).response.rect;
                    ScenePointerChrome::exclude(ctx, header.intersect(ui.clip_rect()));
                });
                scene = panel.response.rect;
                hint = egui::Area::new(egui::Id::new("routing-test-hint")).interactable(false)
                    .fixed_pos(egui::pos2(400.0, 700.0)).show(ctx, |ui| { ui.label("Tool hint"); }).response.rect;
                ScenePointerChrome::exclude(ctx, hint.intersect(screen));
                ScenePointerChrome::finish_pass(ctx, scene, 7);
            });
        }
        let region = CameraPointerRegion { rect: scene, pixels_per_point: scale };
        let physical = |point: egui::Pos2| Vec2::new(point.x, point.y) * scale;
        let standalone = egui::Context::default();
        assert!(region.accepts_pointer(physical(scene.center()), &standalone), "configured standalone regions need no editor publication");
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &standalone), "editor admission explicitly requires publication");
        assert!(ScenePointerChrome::blocks_current_ui(&standalone, scene.center()), "secondary editor admission requires publication");
        assert!(!ctx.is_using_pointer(), "no stale active-widget capture is needed");
        assert!(!region.accepts_editor_pointer(physical(header.center()), &ctx));
        assert!(!region.accepts_editor_pointer(physical(hint.center()), &ctx));
        assert!(region.accepts_editor_pointer(physical(scene.center()), &ctx));
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, header.center()), "secondary clicks respect completed header chrome");
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, hint.center()), "secondary clicks respect completed passive hints");
        assert!(!ScenePointerChrome::blocks_current_ui(&ctx, scene.center()), "ordinary uncovered secondary clicks remain eligible");
        let shifted = CameraPointerRegion { rect: scene.shrink(1.0), pixels_per_point: scale };
        assert!(!shifted.accepts_editor_pointer(physical(scene.center()), &ctx), "different scene geometry invalidates publication");
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "returning geometry cannot revive a rejected snapshot");
        ScenePointerChrome::begin_pass(&ctx, 7);
        ScenePointerChrome::exclude(&ctx, header);
        ScenePointerChrome::finish_pass(&ctx, scene, 7);
        ScenePointerChrome::begin_pass(&ctx, 7);
        assert!(!region.accepts_editor_pointer(physical(header.center()), &ctx), "an unfinished empty build keeps completed chrome");
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, header.center()), "pending build keeps completed secondary chrome");
        assert!(!ScenePointerChrome::blocks_current_ui(&ctx, scene.center()), "a valid pending pass retains completed uncovered admission");
        ScenePointerChrome::exclude(&ctx, egui::Rect::from_center_size(scene.center(), egui::vec2(20.0, 20.0)));
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, scene.center()), "current pending chrome blocks secondary clicks too");
        for _ in 0..=CHROME_CAPACITY { ScenePointerChrome::exclude(&ctx, header); }
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, scene.center() + egui::vec2(50.0, 0.0)), "invalid pending overflow blocks all secondary admission");
        ScenePointerChrome::finish_pass(&ctx, scene, 7);
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "overflow fails closed");
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, scene.center()), "invalid completed publication blocks secondary admission");
        ScenePointerChrome::begin_pass(&ctx, 7);
        ScenePointerChrome::exclude(&ctx, header);
        ScenePointerChrome::finish_pass(&ctx, scene, 7);
        assert!(region.accepts_editor_pointer(physical(scene.center()), &ctx));
        ctx.set_pixels_per_point(scale * 1.25);
        let _output = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |_| {});
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "changed egui scale rejects old painted geometry");
        ctx.set_pixels_per_point(scale);
        let _output = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |_| {});
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "returning scale cannot resurrect completion");
        ScenePointerChrome::observe(&ctx, stamp);
        ScenePointerChrome::begin_pass(&ctx, 7);
        ScenePointerChrome::exclude(&ctx, header);
        ScenePointerChrome::finish_pass(&ctx, scene, 7);
        ScenePointerChrome::begin_pass(&ctx, 7);
        ScenePointerChrome::exclude(&ctx, egui::Rect::from_min_max(egui::pos2(f32::NAN, 0.0), egui::pos2(1.0, 1.0)));
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, scene.center()), "invalid pending geometry blocks secondary admission");
        ScenePointerChrome::finish_pass(&ctx, scene, 7);
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "invalid geometry fails closed");
        ScenePointerChrome::begin_pass(&ctx, 7);
        ScenePointerChrome::exclude(&ctx, header);
        ScenePointerChrome::finish_pass(&ctx, scene, 7);
        for changed in [
            ScenePointerStamp::new(Entity::from_bits(2), stamp.physical_size, 1.0, 7),
            ScenePointerStamp::new(stamp.window, stamp.physical_size + UVec2::ONE, 1.0, 7),
            ScenePointerStamp::new(stamp.window, stamp.physical_size, 2.0, 7),
            ScenePointerStamp::new(stamp.window, stamp.physical_size, 1.0, 8),
        ] {
            ScenePointerChrome::observe(&ctx, changed);
            assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "changed window/geometry/scope cannot reuse chrome");
        }
        ScenePointerChrome::observe(&ctx, stamp);
        ScenePointerChrome::begin_pass(&ctx, 7);
        ScenePointerChrome::exclude(&ctx, header);
        ScenePointerChrome::finish_pass(&ctx, scene, 8);
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "navigation during paint invalidates publication");
        ScenePointerChrome::invalidate(&ctx);
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &ctx), "absent snapshot never means empty chrome");
        assert!(ScenePointerChrome::blocks_current_ui(&ctx, scene.center()), "invalidated context blocks secondary admission");
        assert!(!region.accepts_editor_pointer(physical(scene.center()), &egui::Context::default()), "foreign context has no matching publication");
    }
}
