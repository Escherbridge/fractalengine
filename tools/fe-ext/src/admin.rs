//! `fe-ext admin <verb>`: drives the running app's local admin channel (docs/extensions/admin-channel.md §8).
//! Local channel only: no network fallback, no token or URL flags, and consent is answered in the app, never here.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use fe_plugin::admin::protocol::{
    AdminReply, AdminRequest, AdminResult, AdminVerb, InstallationSummary, PendingConsent, ReplyOutcome,
};
use fe_plugin::admin::transport::{connect_local, exchange, LocalEndpoint};

/// Arguments of `fe-ext admin`.
#[derive(Debug, Clone, Args)]
pub struct AdminArgs {
    /// Print the final reply frame as JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: AdminCommand,
}

/// One administration verb per subcommand.
#[derive(Debug, Clone, Subcommand)]
pub enum AdminCommand {
    /// Every installation on this host.
    List,
    /// One installation.
    Status { installation_id: String },
    /// Verify and stage a package file (may open the consent dialog in the app).
    InstallFile { path: PathBuf },
    /// Fetch, verify and stage a package from a configured registry source (may open the consent dialog).
    InstallRegistry {
        source_id: String,
        package_id: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Switch one binding on, or every binding of the installation.
    Enable {
        installation_id: String,
        #[arg(long)]
        binding: Option<String>,
    },
    /// Switch one binding off, or every binding of the installation.
    Disable {
        installation_id: String,
        #[arg(long)]
        binding: Option<String>,
    },
    /// Stage the newer version (a changed capability digest opens the consent dialog).
    Update { installation_id: String },
    /// Withdraw this host's local acceptance of the installation in one verse.
    Unbind { installation_id: String, verse_id: String },
    /// Remove the installation.
    Uninstall { installation_id: String },
    /// The runtime host switch: stop every extension.
    DisableAll,
    /// Clear the runtime host switch.
    EnableAll,
}

/// Entry point for `fe-ext admin`; exits non-zero when the app refuses or is not running.
pub fn run_admin(args: AdminArgs) -> Result<()> {
    let request = build_request(&args.command, &request_id())?;
    let endpoint = LocalEndpoint::from_env().map_err(|error| anyhow!("{error} (FE_EXT_ADMIN_INSTANCE)"))?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().context("start the async runtime")?;
    let reply = runtime.block_on(async {
        let mut stream = connect_local(&endpoint).await?;
        let mut stderr = std::io::stderr();
        exchange(&mut stream, &request, |pending| {
            let _ = writeln!(stderr, "{}", pending_notice(pending));
        })
        .await
        .map_err(anyhow::Error::from)
    })?;
    let refused = matches!(reply.outcome, ReplyOutcome::Refused { .. });
    if args.json {
        println!("{}", render_json(&reply)?);
    } else if refused {
        eprintln!("{}", render_human(&reply));
    } else {
        println!("{}", render_human(&reply));
    }
    if refused {
        std::process::exit(1);
    }
    Ok(())
}

/// Correlation id of this invocation (`A-Z a-z 0-9 _ -`, at most 64 characters).
fn request_id() -> String {
    format!("fe-ext-{}", std::process::id())
}

/// Turn the parsed subcommand into a validated request; relative `install-file` paths become absolute.
pub fn build_request(command: &AdminCommand, id: &str) -> Result<AdminRequest> {
    let verb = match command {
        AdminCommand::List => AdminVerb::List {},
        AdminCommand::Status { installation_id } => AdminVerb::Status { installation_id: installation_id.clone() },
        AdminCommand::InstallFile { path } => {
            let absolute = std::path::absolute(path).with_context(|| format!("resolve {}", path.display()))?;
            let path = absolute.to_str().ok_or_else(|| anyhow!("the package path is not valid UTF-8"))?.to_owned();
            AdminVerb::InstallFile { path }
        }
        AdminCommand::InstallRegistry { source_id, package_id, version } => AdminVerb::InstallRegistry {
            source_id: source_id.clone(),
            package_id: package_id.clone(),
            version: version.clone(),
        },
        AdminCommand::Enable { installation_id, binding } => {
            AdminVerb::Enable { installation_id: installation_id.clone(), binding_id: binding.clone() }
        }
        AdminCommand::Disable { installation_id, binding } => {
            AdminVerb::Disable { installation_id: installation_id.clone(), binding_id: binding.clone() }
        }
        AdminCommand::Update { installation_id } => AdminVerb::Update { installation_id: installation_id.clone() },
        AdminCommand::Unbind { installation_id, verse_id } => {
            AdminVerb::Unbind { installation_id: installation_id.clone(), verse_id: verse_id.clone() }
        }
        AdminCommand::Uninstall { installation_id } => AdminVerb::Uninstall { installation_id: installation_id.clone() },
        AdminCommand::DisableAll => AdminVerb::DisableAll {},
        AdminCommand::EnableAll => AdminVerb::EnableAll {},
    };
    if let Err(reason) = verb.validate() {
        bail!("invalid input: {reason}");
    }
    Ok(AdminRequest::new(id, verb))
}

/// The line printed on stderr while the operator decides in the app.
pub fn pending_notice(pending: &PendingConsent) -> String {
    let kind = if pending.is_update { "update" } else { "install" };
    let digest: String = pending.capability_digest.chars().take(12).collect();
    format!(
        "Waiting for approval of {kind} {} {} (capabilities {digest}). Review the request in the FractalEngine window; this command finishes when you answer there.",
        pending.name, pending.version
    )
}

/// The final reply frame as JSON.
pub fn render_json(reply: &AdminReply) -> Result<String> {
    serde_json::to_string(reply).context("encode the reply")
}

/// The final reply as text: an installation table for `ok`, `refused: <code>: <message>` otherwise.
pub fn render_human(reply: &AdminReply) -> String {
    match &reply.outcome {
        ReplyOutcome::Ok { result } => render_result(result),
        ReplyOutcome::PendingConsent { consent } => pending_notice(consent),
        ReplyOutcome::Refused { code, message } => {
            if message.is_empty() {
                format!("refused: {}", code.as_str())
            } else {
                format!("refused: {}: {message}", code.as_str())
            }
        }
    }
}

fn render_result(result: &AdminResult) -> String {
    let mut lines = Vec::new();
    if let Some(message) = &result.message {
        lines.push(message.clone());
    }
    for installation in &result.installations {
        lines.push(render_installation(installation));
        for binding in &installation.bindings {
            let state = if binding.enabled { "on" } else { "off" };
            lines.push(format!("    {} [{state}] {}", binding.binding_id, binding.label));
        }
    }
    if lines.is_empty() {
        lines.push("ok".to_owned());
    }
    lines.join("\n")
}

fn render_installation(installation: &InstallationSummary) -> String {
    let update = installation.update_available.as_deref().map(|version| format!(" (update {version} available)")).unwrap_or_default();
    format!(
        "{}  {} {} {}  {}{update}",
        installation.installation_id, installation.name, installation.version, installation.package_id, installation.status
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use fe_plugin::admin::protocol::{
        decode_request, encode_reply, read_frame, write_frame, BindingSummary, RefusalCode,
    };

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        admin: AdminArgs,
    }

    fn request_for(line: &[&str]) -> Result<AdminRequest> {
        let mut argv = vec!["admin"];
        argv.extend_from_slice(line);
        let parsed = Harness::try_parse_from(argv).map_err(|error| anyhow!("{error}"))?;
        build_request(&parsed.admin.command, "t1")
    }

    #[test]
    fn arguments_map_to_verbs_and_reject_bad_input() {
        let cases: Vec<(Vec<&str>, Option<AdminVerb>)> = vec![
            (vec!["list"], Some(AdminVerb::List {})),
            (vec!["disable-all"], Some(AdminVerb::DisableAll {})),
            (
                vec!["enable", "inst1", "--binding", "b1"],
                Some(AdminVerb::Enable { installation_id: "inst1".into(), binding_id: Some("b1".into()) }),
            ),
            (
                vec!["install-registry", "main", "org.example.fleet-sim", "--version", "1.2.0"],
                Some(AdminVerb::InstallRegistry {
                    source_id: "main".into(),
                    package_id: "org.example.fleet-sim".into(),
                    version: Some("1.2.0".into()),
                }),
            ),
            (vec!["unbind", "inst1", "verse1"], Some(AdminVerb::Unbind { installation_id: "inst1".into(), verse_id: "verse1".into() })),
            // No network or credential flags exist.
            (vec!["list", "--token", "x"], None),
            (vec!["list", "--url", "http://x"], None),
            // Invalid ids are caught before anything is sent.
            (vec!["status", "has space"], None),
            (vec!["install-registry", "main", "bad.json"], None),
            (vec!["frobnicate"], None),
        ];
        for (line, expected) in cases {
            let outcome = request_for(&line);
            match expected {
                Some(verb) => assert_eq!(outcome.unwrap().verb, verb, "{line:?}"),
                None => assert!(outcome.is_err(), "{line:?} must be refused"),
            }
        }
    }

    #[test]
    fn install_file_sends_an_absolute_path() {
        let request = request_for(&["install-file", "pkg/fleet-sim.hexon"]).unwrap();
        let AdminVerb::InstallFile { path } = request.verb else { panic!("wrong verb") };
        assert!(std::path::Path::new(&path).is_absolute(), "{path}");
        assert!(path.ends_with("fleet-sim.hexon"));
    }

    #[test]
    fn replies_render_for_humans_and_as_json() {
        let ok = AdminReply::ok(
            "t1",
            AdminResult {
                message: None,
                installations: vec![InstallationSummary {
                    installation_id: "inst1".into(),
                    package_id: "org.example.fleet-sim".into(),
                    name: "Fleet sim".into(),
                    version: "1.2.0".into(),
                    status: "Running".into(),
                    update_available: Some("1.3.0".into()),
                    bindings: vec![BindingSummary { binding_id: "b1".into(), label: "Fleet feed".into(), enabled: true }],
                }],
            },
        );
        let text = render_human(&ok);
        assert!(text.contains("Fleet sim 1.2.0 org.example.fleet-sim  Running (update 1.3.0 available)"), "{text}");
        assert!(text.contains("b1 [on] Fleet feed"), "{text}");
        let json: serde_json::Value = serde_json::from_str(&render_json(&ok).unwrap()).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["id"], "t1");

        let refused = AdminReply::refused("t1", RefusalCode::ConsentDeclined, "Declined in the app");
        assert_eq!(render_human(&refused), "refused: consent-declined: Declined in the app");
        assert_eq!(render_human(&AdminReply::ok("t1", AdminResult::default())), "ok");
    }

    #[test]
    fn pending_consent_then_decision_over_a_stream() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (mut client, mut server) = tokio::io::duplex(64 * 1024);
            let app = tokio::spawn(async move {
                let body = read_frame(&mut server).await.unwrap().unwrap();
                let request = decode_request(&body).unwrap();
                let pending = PendingConsent {
                    installation_id: "inst1".into(),
                    package_id: "org.example.fleet-sim".into(),
                    name: "Fleet sim".into(),
                    version: "1.2.0".into(),
                    capability_digest: "b71e0000000000000000".into(),
                    is_update: false,
                };
                write_frame(&mut server, &encode_reply(&AdminReply::pending_consent(request.id.clone(), pending))).await.unwrap();
                write_frame(&mut server, &encode_reply(&AdminReply::refused(request.id, RefusalCode::ConsentDeclined, "Declined in the app")))
                    .await
                    .unwrap();
            });
            let request = AdminRequest::new("t1", AdminVerb::InstallFile { path: "/tmp/x.hexon".into() });
            let mut notices = Vec::new();
            let reply = exchange(&mut client, &request, |pending| notices.push(pending_notice(pending))).await.unwrap();
            app.await.unwrap();
            assert_eq!(notices.len(), 1);
            assert!(notices[0].contains("Review the request in the FractalEngine window"), "{}", notices[0]);
            assert!(matches!(reply.outcome, ReplyOutcome::Refused { code: RefusalCode::ConsentDeclined, .. }));
        });
    }
}
