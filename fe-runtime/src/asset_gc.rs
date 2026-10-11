//! Periodic unreferenced-asset GC trigger — see `src/AGENTS.md` §asset-gc.

use std::time::Duration;

use bevy::prelude::*;

use crate::app::DbCommandSender;
use crate::messages::DbCommand;

/// Env: seconds between GC passes (default 3600; `0` disables the timer).
pub const ASSET_GC_INTERVAL_ENV: &str = "FE_ASSET_GC_INTERVAL_SECS";
/// Env: minimum age in seconds of an unreferenced asset before GC deletes it (default 86400).
pub const ASSET_GC_GRACE_ENV: &str = "FE_ASSET_GC_GRACE_SECS";
/// Default seconds between GC passes.
pub const DEFAULT_ASSET_GC_INTERVAL_SECS: u64 = 3_600;
/// Default grace before an unreferenced upload is collectable.
pub const DEFAULT_ASSET_GC_GRACE_SECS: u64 = 86_400;

/// GC cadence + grace, read from the environment once at startup.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetGcSchedule {
    /// `None` disables the timer.
    pub interval: Option<Duration>,
    pub grace_secs: u64,
}

impl AssetGcSchedule {
    /// Read [`ASSET_GC_INTERVAL_ENV`] / [`ASSET_GC_GRACE_ENV`]; unset or
    /// unparseable values fall back to the defaults (with a warning).
    pub fn from_env() -> Self {
        let read = |key: &str, default: u64| match std::env::var(key) {
            Ok(v) => v.trim().parse().unwrap_or_else(|_| {
                warn!("{key}={v:?} is not a whole number of seconds; using {default}");
                default
            }),
            Err(_) => default,
        };
        let interval_secs = read(ASSET_GC_INTERVAL_ENV, DEFAULT_ASSET_GC_INTERVAL_SECS);
        Self {
            interval: (interval_secs > 0).then(|| Duration::from_secs(interval_secs)),
            grace_secs: read(ASSET_GC_GRACE_ENV, DEFAULT_ASSET_GC_GRACE_SECS),
        }
    }
}

/// Every `interval`, send a fire-and-forget `GcUnreferencedAssets` to the DB
/// thread (no waiter is registered; a full/closed channel is only logged).
pub fn schedule_asset_gc(
    schedule: Res<AssetGcSchedule>,
    time: Res<Time>,
    db_sender: Res<DbCommandSender>,
    mut timer: Local<Option<Timer>>,
) {
    let Some(interval) = schedule.interval else {
        return;
    };
    let timer = timer.get_or_insert_with(|| Timer::new(interval, TimerMode::Repeating));
    if !timer.tick(time.delta()).just_finished() {
        return;
    }
    let cmd = DbCommand::GcUnreferencedAssets {
        grace_secs: schedule.grace_secs,
    };
    if let Err(e) = db_sender.0.try_send(cmd) {
        warn!("asset GC trigger not sent to the DB thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;

    fn app(schedule: AssetGcSchedule) -> (App, crossbeam::channel::Receiver<DbCommand>) {
        let (db_tx, db_rx) = crossbeam::channel::bounded(8);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .insert_resource(DbCommandSender(db_tx))
            .insert_resource(schedule)
            .add_systems(Update, schedule_asset_gc);
        (app, db_rx)
    }

    #[test]
    fn timer_sends_gc_with_the_configured_grace_each_interval() {
        let interval = Duration::from_millis(300);
        let (mut app, db_rx) = app(AssetGcSchedule {
            interval: Some(interval),
            grace_secs: 42,
        });
        for _ in 0..12 {
            app.update();
        }
        // Virtual time clamps each frame's delta, so derive the expectation
        // from the clock the system actually ticked against.
        let elapsed = app.world().resource::<Time>().elapsed();
        let expected = (elapsed.as_millis() / interval.as_millis()) as usize;
        assert!(
            expected >= 2,
            "test must span several intervals ({elapsed:?})"
        );
        let sent: Vec<DbCommand> = db_rx.try_iter().collect();
        assert_eq!(
            sent.len(),
            expected,
            "one trigger per elapsed interval: {sent:?}"
        );
        assert!(sent
            .iter()
            .all(|c| matches!(c, DbCommand::GcUnreferencedAssets { grace_secs: 42 })));
    }
}
