//! Reactive render loop with event-driven wakes (QA-0119); see src/AGENTS.md §render-loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::*;
use bevy::window::RequestRedraw;
use bevy::winit::{EventLoopProxyWrapper, UpdateMode, WinitSettings, WinitUserEvent};

/// Longest idle wait while focused: bounds deadline/timer-driven systems.
const FOCUSED_WAIT: Duration = Duration::from_millis(250);
/// Longest idle wait while unfocused.
const UNFOCUSED_WAIT: Duration = Duration::from_secs(1);
/// How often the waker thread checks the cross-thread queues.
const POLL: Duration = Duration::from_millis(4);

type Probe = Box<dyn Fn() -> bool + Send + Sync>;

/// Cross-thread queues that feed Bevy systems; any non-empty one wakes the loop.
#[derive(Resource, Clone, Default)]
pub struct PendingWork(Arc<Mutex<Vec<Probe>>>);

impl PendingWork {
    /// Wake the loop whenever `receiver` holds a message (the probe never consumes).
    pub fn watch<T: Send + 'static>(&self, receiver: crossbeam::channel::Receiver<T>) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Box::new(move || !receiver.is_empty()));
    }

    fn any(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|probe| probe())
    }
}

/// Frames started; the waker sends at most one wake per frame.
#[derive(Resource, Clone, Default)]
struct FrameCount(Arc<AtomicU64>);

/// Reactive update modes for every build, the waker thread and the keep-awake system.
/// `FE_QA_A11Y_CONTINUOUS=1` still opts into a continuous loop (qa_a11y_timing).
pub fn install(app: &mut App, work: PendingWork) {
    app.insert_resource(WinitSettings {
        focused_mode: UpdateMode::reactive(FOCUSED_WAIT),
        unfocused_mode: UpdateMode::reactive_low_power(UNFOCUSED_WAIT),
    })
    .insert_resource(work)
    .init_resource::<FrameCount>()
    .add_systems(Startup, spawn_waker)
    .add_systems(First, count_frame)
    .add_systems(Last, keep_awake_while_working);
}

fn count_frame(frames: Res<FrameCount>) {
    frames.0.fetch_add(1, Ordering::Relaxed);
}

/// Waker thread: a queued DB result, sync event, API command or bridge
/// request wakes a sleeping loop at once instead of after the idle wait.
fn spawn_waker(proxy: Option<Res<EventLoopProxyWrapper>>, work: Res<PendingWork>, frames: Res<FrameCount>) {
    let Some(proxy) = proxy else { return };
    let proxy = (**proxy).clone();
    let work = work.clone();
    let frames = frames.0.clone();
    let spawned = std::thread::Builder::new()
        .name("render-wake".into())
        .spawn(move || {
            let mut woken_at = u64::MAX;
            loop {
                std::thread::sleep(POLL);
                let frame = frames.load(Ordering::Relaxed);
                if frame == woken_at || !work.any() {
                    continue;
                }
                if proxy.send_event(WinitUserEvent::WakeUp).is_err() {
                    return;
                }
                woken_at = frame;
            }
        });
    if let Err(error) = spawned {
        warn!("render wake thread unavailable; background work waits for the idle tick: {error}");
    }
}

/// Keep frames coming while work is queued or a multi-frame bridge job (input
/// replay, posed capture, screenshot) is in flight, so no job stalls asleep.
fn keep_awake_while_working(
    work: Res<PendingWork>,
    input: Option<Res<crate::input_bridge::InputJob>>,
    vision: Option<Res<crate::vision_bridge::VisionJob>>,
    screenshots: Option<Res<crate::screenshot_bridge::PendingScreenshots>>,
    captures: Query<(), With<bevy::render::view::screenshot::Screenshot>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let busy = input.is_some_and(|job| job.is_active())
        || vision.is_some_and(|job| job.is_active())
        || screenshots.is_some_and(|pending| !pending.is_empty())
        || !captures.is_empty()
        || work.any();
    if busy {
        redraw.write(RequestRedraw);
    }
}
