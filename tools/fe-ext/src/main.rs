//! `fe-ext` command line. Commands map one-to-one onto the library modules.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use fe_ext::{inspect, key, lab, new, pack, project::Project, sandbox, validate};

#[derive(Parser)]
#[command(name = "fe-ext", version, about = "Author, check, pack, sign and sandbox-run FractalEngine extensions")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a guest crate, manifest v2, a fixture and a journey test.
    New {
        #[arg(value_enum)]
        kind: new::Kind,
        /// A reverse-DNS id (`org.acme.thing`) or a short name (becomes `local.<name>`).
        name: String,
        /// Parent directory for the new project.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Compile the component for wasm32-wasip2 and check it against the 2.0 world.
    Build {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Build without --release.
        #[arg(long)]
        debug: bool,
    },
    /// Run the manifest table and host ceilings, plus the component checks when one is built.
    Validate {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        component: Option<PathBuf>,
    },
    /// Build the sealed, signed plugin hexon.
    Pack {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Signing key file (see `keygen`).
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        publisher_name: Option<String>,
        #[arg(long)]
        component: Option<PathBuf>,
    },
    /// Re-seal a package under a different publisher key.
    Sign {
        package: PathBuf,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Add a counter-signature; the sealed members and content hash are unchanged.
    Countersign {
        package: PathBuf,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print identity, signer, content hash and capability digest.
    Inspect {
        package: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Run the component in the real host: `--sandbox` (one peer, fixture) or `--lab` (fe-sim scenario).
    Run {
        /// In-memory scene, manual clock, fixture-fed.
        #[arg(long, conflicts_with = "lab")]
        sandbox: bool,
        /// Host the fleet-sim package as the fe-sim reading schedule; needs `--scenario`.
        #[arg(long, requires = "scenario")]
        lab: bool,
        /// Sandbox fixture (required with `--sandbox`).
        #[arg(long)]
        fixture: Option<PathBuf>,
        /// fe-sim scenario script (JSON) for `--lab`.
        #[arg(long)]
        scenario: Option<PathBuf>,
        /// Parent for the lab session directories (default: a pid-named directory under the OS temp dir).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Lab sessions to run back to back; every fingerprint must match the first.
        #[arg(long, default_value_t = 1)]
        runs: u32,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        component: Option<PathBuf>,
        /// Write the full trace (sandbox) or report (lab) JSON here.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Create a signing key file.
    Keygen {
        out: PathBuf,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Cmd::New { kind, name, dir } => {
            let created = new::scaffold(kind, &name, &dir)?;
            println!("created {}", created.display());
            println!("next: cd {} && fe-ext build && fe-ext run --sandbox --fixture fixtures/basic.json", created.display());
        }
        Cmd::Build { project, debug } => build(&project, debug)?,
        Cmd::Validate { project, component } => {
            let project = Project::open(&project)?;
            let validated = validate::validate_manifest(&project.read_manifest()?)?;
            println!(
                "ok {}@{} kind={} capability_digest={}",
                validated.manifest.id,
                validated.manifest.version,
                serde_json::to_value(&validated.manifest.kind)?.as_str().unwrap_or("?"),
                validated.capability_digest
            );
            let component_path = component.unwrap_or_else(|| project.component_path());
            if component_path.is_file() {
                let digest = validate::validate_component(&std::fs::read(&component_path)?)?;
                println!("component ok digest={digest}");
            }
        }
        Cmd::Pack { project, key: key_path, out, publisher_name, component } => {
            let project = Project::open(&project)?;
            let signing_key = key::load(&key_path)?;
            let (bytes, package) = pack::pack(
                &project.read_manifest()?,
                &project.read_component(component.as_deref())?,
                &signing_key,
                publisher_name.as_deref(),
            )?;
            let out = out.unwrap_or_else(|| project.dir.join("dist").join(format!("{}-{}.hexon", package.package_id, package.version)));
            write_output(&out, &bytes)?;
            println!("packed {}", out.display());
            println!("content_hash {}", package.content_hash);
            println!("component_digest {}", package.component_digest);
        }
        Cmd::Sign { package, key: key_path, out } => {
            let (bytes, signed) = pack::resign(&std::fs::read(&package)?, &key::load(&key_path)?)?;
            let out = out.unwrap_or(package);
            write_output(&out, &bytes)?;
            println!("signed {} content_hash {}", out.display(), signed.content_hash);
        }
        Cmd::Countersign { package, key: key_path, out } => {
            let bytes = pack::countersign(&std::fs::read(&package)?, &key::load(&key_path)?)?;
            let out = out.unwrap_or(package);
            write_output(&out, &bytes)?;
            println!("countersigned {}", out.display());
        }
        Cmd::Inspect { package, json } => {
            let facts = inspect::inspect(&std::fs::read(&package)?)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&facts)?);
            } else if let Some(object) = facts.as_object() {
                for (name, value) in object {
                    println!("{name} {}", value.as_str().map_or_else(|| value.to_string(), str::to_string));
                }
            }
        }
        Cmd::Run { sandbox: sandboxed, lab: labbed, fixture, scenario, root, runs, project, component, out } => {
            if labbed {
                let scenario = scenario.context("`--lab` needs `--scenario <script.json>`")?;
                let report = lab::run_lab(&lab::LabArgs { scenario, project, component, root, runs, out })?;
                for line in &report.summary {
                    println!("{line}");
                }
                return Ok(());
            }
            if !sandboxed {
                bail!("pass `--sandbox --fixture <file>` or `--lab --scenario <file>`");
            }
            let fixture = fixture.context("`--sandbox` needs `--fixture <file>`")?;
            let report = sandbox::run_project(&project, &fixture, component.as_deref())?;
            for line in &report.summary {
                println!("{line}");
            }
            if let Some(out) = out {
                write_output(&out, serde_json::to_string_pretty(&report.trace)?.as_bytes())?;
            }
            if report.trace["faults"].as_array().is_some_and(|faults| !faults.is_empty()) {
                bail!("the run recorded hook faults (see the trace)");
            }
        }
        Cmd::Keygen { out } => println!("{}", key::generate(&out)?),
    }
    Ok(())
}

fn write_output(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// `cargo build --target wasm32-wasip2`, copy the artifact to `dist/component.wasm`, then admit it.
fn build(project_dir: &Path, debug: bool) -> Result<()> {
    let project = Project::open(project_dir)?;
    validate::validate_manifest(&project.read_manifest()?)?;
    let manifest_path = project.dir.join("Cargo.toml");
    // An explicit target dir: a CARGO_TARGET_DIR or config `target-dir` would otherwise move the artifact.
    let target_dir = project.dir.join("target");
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.args(["build", "--target", "wasm32-wasip2", "--manifest-path"]).arg(&manifest_path).arg("--target-dir").arg(&target_dir);
    if !debug {
        cargo.arg("--release");
    }
    let status = cargo.status().context("running cargo (is the wasm32-wasip2 target installed? `rustup target add wasm32-wasip2`)")?;
    if !status.success() {
        bail!("cargo build failed");
    }
    let profile = if debug { "debug" } else { "release" };
    let artifact = target_dir
        .join("wasm32-wasip2")
        .join(profile)
        .join(format!("{}.wasm", project.crate_name()?.replace('-', "_")));
    let bytes = std::fs::read(&artifact).with_context(|| format!("reading {}", artifact.display()))?;
    let digest = validate::validate_component(&bytes)?;
    write_output(&project.component_path(), &bytes)?;
    println!("built {} ({} bytes, digest {digest})", project.component_path().display(), bytes.len());
    Ok(())
}
