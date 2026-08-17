//! Router assembly for the SPEC-7 `/ws/canonical` module — unmounted by design; see `AGENTS.md`.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use crate::canonical_ws::handler::canonical_ws_handler;
use crate::canonical_ws::state::CanonicalLogState;
use crate::server::ApiState;

/// Builds the existing router unchanged (`crate::server::build_router`), optionally nesting
/// the canonical-log WebSocket surface at `/ws/canonical`.
///
/// `canonical` is `None` in every binary today — no `fractalengine` or `fractalengine-relay`
/// startup file calls this function at all yet, so the endpoint compiles but is not reachable
/// from any running process. See `AGENTS.md` §deliberately-unmounted.
pub fn build_router_with_canonical_log(
    state: Arc<ApiState>,
    canonical: Option<Arc<CanonicalLogState>>,
) -> Router {
    let router = crate::server::build_router(state);
    match canonical {
        Some(canonical) => router.nest(
            "/ws/canonical",
            Router::new()
                .route("/", get(canonical_ws_handler))
                .with_state(canonical),
        ),
        None => router,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use fe_canonical_log::wire::preview_limiter::{PreviewRateLimit, PreviewRateLimiter};
    use fe_canonical_log::wire::test_support::{
        InMemoryBranchRegistry, MockCapabilityVerifier, MockScopeSnapshotSource,
        ScriptedCommitPipeline,
    };

    use super::*;

    /// A path no route in `build_router` defines, used as the "certainly absent" control.
    const ABSENT_CONTROL_PATH: &str = "/ws/canonical-control-absent";

    /// The status `router` answers a plain GET of `path` with.
    async fn status_of(router: axum::Router, path: &str) -> StatusCode {
        router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot")
            .status()
    }

    /// A minimal, DB-free `ApiState` — this test never dispatches an authenticated route, it
    /// only checks whether `/ws/canonical` exists in the router shape.
    fn minimal_api_state() -> Arc<ApiState> {
        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key: ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap(),
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: None,
            cors_origins: vec![],
            db_reader: None,
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
            share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
        })
    }

    fn minimal_canonical_state() -> Arc<CanonicalLogState> {
        struct FixedAuthorizationView;
        impl fe_canonical_log::capability::AuthorizationView for FixedAuthorizationView {
            fn current_epoch(
                &self,
                _epoch_scope: &fe_canonical_log::envelope::Scope,
            ) -> Option<u64> {
                Some(1)
            }
            fn version(&self) -> u64 {
                1
            }
        }

        let registry = Arc::new(InMemoryBranchRegistry::new());
        Arc::new(CanonicalLogState::new(
            Arc::new(ScriptedCommitPipeline {
                derived_op_id: fe_canonical_log::envelope::Hash32([0; 32]),
                result:
                    fe_canonical_log::wire::commit::PipelineResult::AcceptedPendingMaterialization,
            }),
            registry.clone(),
            Arc::new(MockScopeSnapshotSource::new(registry, Vec::new())),
            Arc::new(MockCapabilityVerifier::new()),
            Arc::new(FixedAuthorizationView),
            PreviewRateLimiter::new(PreviewRateLimit::new(10, 1_000)),
            4,
            4,
        ))
    }

    /// `git diff` proves `crate::server::build_router` itself is untouched (acceptance
    /// criteria); this test proves the WRAPPER calls it unchanged: every existing public route
    /// still resolves exactly as it does through `build_router` directly, `None` never adds
    /// `/ws/canonical`, and `Some` does.
    #[tokio::test]
    async fn none_produces_exactly_the_existing_router_with_no_canonical_route() {
        let state = minimal_api_state();
        let router = build_router_with_canonical_log(state.clone(), None);

        let health = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(
            health.status(),
            StatusCode::OK,
            "existing routes still work"
        );

        // `build_router`'s authenticated sub-router applies `auth_middleware` as a `.layer()`,
        // which runs ahead of the 404 fallback, so an unrouted path answers 401 rather than
        // 404. Status alone therefore cannot tell "unmounted" from "mounted but unauthorized";
        // the discriminator is whether `/ws/canonical` is treated like a path that certainly
        // does not exist.
        let canonical = status_of(router.clone(), "/ws/canonical").await;
        let absent = status_of(router, ABSENT_CONTROL_PATH).await;
        assert_eq!(
            canonical, absent,
            "with None, /ws/canonical must be handled exactly like a nonexistent path"
        );
    }

    #[tokio::test]
    async fn some_nests_the_canonical_route_without_disturbing_the_existing_router() {
        let state = minimal_api_state();
        let canonical = minimal_canonical_state();
        let router = build_router_with_canonical_log(state, Some(canonical));

        let health = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("oneshot");
        assert_eq!(health.status(), StatusCode::OK);

        // No `Upgrade: websocket` header, so axum's `WebSocketUpgrade` extractor rejects the
        // request — but it must reject it as the real handler, not as an unrouted path.
        // Comparing against a certainly-absent path is what makes this discriminating:
        // asserting merely `!= NOT_FOUND` would also hold for an unmounted route, because the
        // auth layer answers 401 before the 404 fallback (see the `None` test above).
        let mounted = status_of(router.clone(), "/ws/canonical").await;
        let absent = status_of(router, ABSENT_CONTROL_PATH).await;
        assert_ne!(
            mounted, absent,
            "with Some, /ws/canonical must be handled by its own handler, not the fallback"
        );
    }
}
