//! Captured run-local browser capability; see src/AGENTS.md section QA web fixtures.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, ensure, Context};
use serde_json::Value;

#[cfg(windows)]
#[path = "qa_web/windows.rs"]
mod platform;
#[cfg(not(windows))]
#[path = "qa_web/unsupported.rs"]
mod platform;

const SCHEMA: &str = "fe-qa-controlled-fixtures-v1";
const MAX_RECEIPT: u64 = 32 * 1024;
static CAPABILITY: OnceLock<Option<Permit>> = OnceLock::new();

struct Permit {
    urls: [String; 2],
    helper: platform::Helper,
    deadline: Instant,
    expires: SystemTime,
    revoked: AtomicBool,
    _pins: Vec<File>,
    _receipts: Vec<File>,
}

/// Capture once before DB/browser startup; absence leaves production policy intact.
pub fn install(root: Option<&Path>, input_enabled: bool) -> anyhow::Result<()> {
    let captured = match (root, input_enabled) {
        (Some(root), true) if root.join("controlled-fixtures.json").exists() => {
            Permit::capture(root).map(Some)
        }
        _ => Ok(None),
    };
    let (permit, error) = match captured {
        Ok(permit) => (permit, None),
        Err(error) => (None, Some(error)),
    };
    ensure!(
        CAPABILITY.set(permit).is_ok(),
        "QA web capability already captured"
    );
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

/// Cheap advisory only; actual effects must call `allows` again.
pub fn preview_allows(raw: &str) -> bool {
    CAPABILITY
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|permit| {
            permit.urls.iter().any(|url| url == raw)
                && !permit.revoked.load(Ordering::Acquire)
                && Instant::now() < permit.deadline
                && SystemTime::now() < permit.expires
        })
}

/// Revalidate the retained helper and its exact listening socket before an effect.
pub fn allows(raw: &str) -> bool {
    let Some(permit) = CAPABILITY.get().and_then(Option::as_ref) else {
        return false;
    };
    if !preview_allows(raw) {
        return false;
    }
    if permit.helper.is_live_listener() {
        true
    } else {
        permit.revoked.store(true, Ordering::Release);
        false
    }
}

impl Permit {
    fn capture(root: &Path) -> anyhow::Result<Self> {
        ensure!(
            root.is_absolute() && root.canonicalize()? == root,
            "QA root is not canonical"
        );
        ensure!(
            root.parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "run")
                && root
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == ".qa"),
            "QA web requires a direct owned run"
        );
        let mut pins =
            fe_identity::file_fallback::pin_secret_parents(&root.join("controlled-fixtures.json"))?;
        let (current, _) = read_receipt(&root.join("current.json"))?;
        ensure!(
            same_path(string(&current, "run_dir")?, root)?
                && current
                    .pointer("/qa_options/synthetic_input")
                    .and_then(Value::as_bool)
                    == Some(true),
            "QA run input receipt does not match"
        );
        let (record, record_file) = read_receipt(&root.join("controlled-fixtures.json"))?;
        ensure!(
            string(&record, "schema")? == SCHEMA && same_path(string(&record, "run_dir")?, root)?,
            "QA fixture run receipt does not match"
        );
        let instance = string(&record, "instance")?;
        ensure!(
            instance.len() == 32
                && instance
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid QA fixture instance"
        );
        let directory = root.join("controlled-fixtures").join(instance);
        ensure!(
            same_path(string(&record, "instance_dir")?, &directory)?,
            "QA fixture instance directory does not match"
        );
        pins.extend(fe_identity::file_fallback::pin_secret_parents(
            &directory.join("config.json"),
        )?);
        let (config, config_file) = read_receipt(&directory.join("config.json"))?;
        let (ready, ready_file) = read_receipt(&directory.join("ready.json"))?;
        let port = u16::try_from(config["port"].as_u64().context("missing fixture port")?)?;
        let lifetime = config["lifetime_seconds"]
            .as_u64()
            .context("missing fixture lifetime")?;
        ensure!(
            port >= 1024 && (30..=3600).contains(&lifetime),
            "fixture bounds rejected"
        );
        let origin = format!("http://127.0.0.1:{port}");
        for receipt in [&record, &config, &ready] {
            ensure!(
                string(receipt, "schema")? == SCHEMA && string(receipt, "instance")? == instance,
                "fixture receipts disagree"
            );
        }
        ensure!(
            string(&record, "origin")? == origin
                && string(&ready, "origin")? == origin
                && string(&record, "web_url")? == format!("{origin}/web/one")
                && record["lifetime_seconds"].as_u64() == Some(lifetime)
                && record["mode"] == config["mode"]
                && string(&record, "archive_sha256")? == string(&ready, "archive_sha256")?
                && string(&record, "archive_sha256")? == string(&config, "archive_sha256")?
                && string(&current, "api_base")?.trim_end_matches('/') != origin,
            "fixture endpoint receipts disagree"
        );
        let identity = &record["process_identity"];
        let pid = u32::try_from(identity["pid"].as_u64().context("missing helper PID")?)?;
        ensure!(
            pid != 0 && ready["pid"].as_u64() == Some(u64::from(pid)),
            "fixture process receipts disagree"
        );
        let start_ticks = string(identity, "start_ticks")?.parse::<u64>()?;
        let executable = PathBuf::from(string(identity, "exe_path")?);
        ensure!(
            same_path(string(&record, "python_exe")?, &executable)?,
            "fixture executable receipts disagree"
        );
        let (helper, created) = platform::Helper::capture(pid, start_ticks, &executable, port)?;
        let expires = created
            .checked_add(Duration::from_secs(lifetime))
            .context("fixture expiry overflow")?;
        let remaining = expires
            .duration_since(SystemTime::now())
            .context("fixture expired")?;
        ensure!(
            remaining <= Duration::from_secs(lifetime) && helper.is_live_listener(),
            "fixture process/socket unavailable"
        );
        Ok(Self {
            urls: [format!("{origin}/web/one"), format!("{origin}/web/two")],
            helper,
            deadline: Instant::now() + remaining,
            expires,
            revoked: AtomicBool::new(false),
            _pins: pins,
            _receipts: vec![record_file, config_file, ready_file],
        })
    }
}

fn string<'a>(object: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    object[key]
        .as_str()
        .context("missing fixture receipt field")
}

fn same_path(raw: &str, expected: &Path) -> anyhow::Result<bool> {
    let candidate = Path::new(raw);
    if !candidate.is_absolute()
        || !candidate.exists()
        || !expected.exists()
        || candidate
            .components()
            .any(|part| part == std::path::Component::ParentDir)
    {
        return Ok(false);
    }
    let _pins = fe_identity::file_fallback::pin_secret_parents(candidate)?;
    Ok(candidate.canonicalize()? == expected.canonicalize()?)
}

fn read_receipt(path: &Path) -> anyhow::Result<(Value, File)> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= MAX_RECEIPT,
        "invalid fixture receipt file"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "redirected fixture receipt"
        );
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let file = options.open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_RECEIPT,
        "fixture receipt exceeds cap"
    );
    let mut bytes = Vec::new();
    (&file).take(MAX_RECEIPT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECEIPT {
        bail!("fixture receipt exceeds cap");
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let value = serde_json::from_slice(bytes)?;
    Ok((value, file))
}
