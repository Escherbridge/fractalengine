use crate::cache::SessionCache;
use ed25519_dalek::VerifyingKey;
use fe_database::{DbHandle, PetalId, RoleId};
use fe_identity::NodeKeypair;

pub async fn connect(
    peer_pub_key: VerifyingKey,
    petal_id: &PetalId,
    node_keypair: &NodeKeypair,
    db_handle: &DbHandle,
    cache: &mut SessionCache,
) -> anyhow::Result<String> {
    let peer_did = fe_identity::did_key::did_key_from_public_key(&peer_pub_key);
    let petal_id_str = petal_id.0.to_string();
    // Migration bridge: handshake only has petal_id context.
    // Full hierarchical scope will be used once verse/fractal context is available here.
    let scope = format!("VERSE#_-FRACTAL#_-PETAL#{}", petal_id_str);
    let role: RoleId = fe_database::rbac::get_role(&*db_handle.0, &peer_did, &scope)
        .await
        .unwrap_or_else(|_| RoleId("public".to_string()));
    let token = fe_identity::jwt::mint_session_token(node_keypair, &petal_id_str, &role.0, 300)?;
    let key_bytes = peer_pub_key.to_bytes();
    cache.insert(key_bytes, role);
    Ok(token)
}
