use ed25519_dalek::VerifyingKey;
use fe_database::{PetalId, DbHandle, RoleId};
use fe_identity::NodeKeypair;
use crate::cache::SessionCache;

pub async fn connect(
    peer_pub_key: VerifyingKey,
    petal_id: &PetalId,
    node_keypair: &NodeKeypair,
    db_handle: &DbHandle,
    cache: &mut SessionCache,
) -> anyhow::Result<String> {
    let node_id_hex = hex::encode(peer_pub_key.to_bytes());
    let petal_id_str = petal_id.0.to_string();
    let role = fe_database::rbac::get_role(&db_handle.0, &node_id_hex, &petal_id_str)
        .await
        .unwrap_or_else(|_| RoleId("public".to_string()));
    let token = fe_identity::jwt::mint_session_token(
        node_keypair,
        &petal_id_str,
        &role.0,
        300,
    )?;
    let key_bytes = peer_pub_key.to_bytes();
    cache.insert(key_bytes, role);
    Ok(token)
}
