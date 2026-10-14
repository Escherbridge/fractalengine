//! PAS-P5 A7 (HTTP half): the hosted registry passes the partial-fetch
//! conformance suite, and `FetchStrategy` routes around a tampering registry.

#![cfg(all(feature = "remote", feature = "conformance"))]

#[path = "support/registry_server.rs"]
mod registry_server;

use std::sync::Arc;

use fe_hexon::p2p::P2pConfig;
use fe_hexon::partial::conformance::{self, ConformanceFixtures};
use fe_hexon::partial::{FetchStrategy, SharedSource};
use fe_hexon::remote::RemoteRegistryClient;
use registry_server::{spawn_registry, tamper_member};

fn place(dir: &std::path::Path, file_name: &str, bytes: &[u8]) {
    std::fs::write(dir.join(file_name), bytes).expect("fixture write failed");
}

#[tokio::test]
async fn http_source_passes_conformance() {
    let fixtures = ConformanceFixtures::build().expect("fixtures build");
    let dir = tempfile::tempdir().unwrap();
    for fixture in fixtures.all() {
        place(dir.path(), &fixture.file_name(), &fixture.bytes);
    }
    let addr = spawn_registry(dir.path()).await;
    let client: SharedSource = Arc::new(RemoteRegistryClient::new(format!("http://{addr}"), None));

    let report = conformance::run(client, &fixtures).await;

    assert!(report.passed(), "{report}");
    // Non-vacuous: every container chunk and every probe member was exercised.
    for name in [
        "conformance-container: chunk 2 bytes equal the signed bytes",
        "conformance-container: chunk 3 is out of range",
        "conformance-tileset: chunk 0 bytes equal the signed bytes",
        "conformance-model: member terrain/tracks/route.gpx bytes equal the archive's",
        "conformance-model: a pinned wrong publisher is refused",
        "unknown uri: the transport itself answers NotFound",
    ] {
        assert!(
            report.cases.iter().any(|case| case.name == name),
            "missing case {name}: {report}"
        );
    }
}

#[tokio::test]
async fn strategy_falls_back_from_a_tampering_source_to_an_honest_one() {
    let fixtures = ConformanceFixtures::build().expect("fixtures build");
    let honest_fixture = &fixtures.tileset;

    let honest_dir = tempfile::tempdir().unwrap();
    place(
        honest_dir.path(),
        &honest_fixture.file_name(),
        &honest_fixture.bytes,
    );
    // The header stays honest (so /meta verifies); one tile no longer matches
    // its signed digest, and the registry serves the whole file unverified.
    let tampering_dir = tempfile::tempdir().unwrap();
    place(
        tampering_dir.path(),
        &honest_fixture.file_name(),
        &tamper_member(
            &honest_fixture.bytes,
            "terrain/tiles/10/1/0.png",
            b"not the signed tile",
        ),
    );

    let tampering: SharedSource = Arc::new(RemoteRegistryClient::new(
        format!("http://{}", spawn_registry(tampering_dir.path()).await),
        None,
    ));
    let honest: SharedSource = Arc::new(RemoteRegistryClient::new(
        format!("http://{}", spawn_registry(honest_dir.path()).await),
        None,
    ));
    // Same tier, so the caller's order holds: the tampering registry goes first.
    let sources = vec![Arc::clone(&tampering), Arc::clone(&honest)];
    let strategy = FetchStrategy::new(P2pConfig::default());

    let header = strategy
        .fetch_header(
            &honest_fixture.uri(),
            &sources,
            Some(&fixtures.publisher_did),
        )
        .await
        .expect("the tampering registry's header is itself honest");
    assert_eq!(header.served_by, tampering.source());

    let archive = strategy
        .fetch_chunk(&header.value, 0, &sources)
        .await
        .expect("the honest registry serves the archive");
    assert_eq!(archive.value.as_ref(), honest_fixture.bytes.as_slice());
    assert_eq!(archive.served_by, honest.source());
    let demoted: Vec<_> = archive.demoted().cloned().collect();
    assert_eq!(demoted, vec![tampering.source()]);
    assert!(strategy.is_demoted(&tampering.source()));
    assert!(!strategy.is_demoted(&honest.source()));

    // Demotion sticks: the next fetch goes to the honest registry first.
    let again = strategy
        .fetch_chunk(&header.value, 0, &sources)
        .await
        .expect("honest refetch");
    assert_eq!(again.served_by, honest.source());
    assert!(again.attempts.is_empty());
}
