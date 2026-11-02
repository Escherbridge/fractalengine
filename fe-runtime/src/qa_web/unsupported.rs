use std::path::Path;
use std::time::SystemTime;

pub(super) struct Helper;

impl Helper {
    pub(super) fn capture(_: u32, _: u64, _: &Path, _: u16) -> anyhow::Result<(Self, SystemTime)> {
        anyhow::bail!("QA web helper ownership verification is unsupported on this platform")
    }

    pub(super) fn is_live_listener(&self) -> bool {
        false
    }
}
