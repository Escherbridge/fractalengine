pub async fn sync_petal_state(petal_id: &str, peer_id: &str) -> anyhow::Result<Vec<u8>> {
    // CROSS-CRATE: iroh-docs range reconciliation, delta only — Sprint 5B
    tracing::info!("Syncing petal {} state with peer {}", petal_id, peer_id);
    Ok(Vec::new())
}
