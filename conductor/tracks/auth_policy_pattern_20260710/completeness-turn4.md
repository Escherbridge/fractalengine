---
type: Review
track: auth_policy_pattern_20260710
critic: C-DATA
round: 4
---

# C-DATA completeness — auth_policy_pattern_20260710 (round 4)

Checked at public HEAD `733a3256`. No cargo, no runs. No round-4 code slice targeted this track; this verifies the standing claims.

## Spec acceptance criteria and target shape

| Requirement | Status | Evidence |
|---|---|---|
| `Policy::evaluate(subject, action, scope) -> Decision`, pure data | complete | fe-policy/src/engine.rs:8-9 trait; types fe-policy/src/types.rs:11 `AuthContext`, :61 `Action`, :88 `Scope`, :141 `Decision` |
| Deny-by-default, proven with an empty set | complete | engine.rs:46 empty check, tests :133 `empty_engine_denies_by_default`, :144 `empty_engine_denies_even_owner_write` |
| Decision logging (allow debug, deny warn) | complete | engine.rs:82 `tracing::debug!`, :89 `tracing::warn!` |
| Composable AnyOf/AllOf | complete | fe-policy/src/combinators.rs:9, :40, :70 `PermissiveMigrationPolicy` |
| RoleLevel/TokenScope/Capability/Ownership policies | complete | fe-policy/src/policies.rs:8, :66, :83, :107 `RoleFloorPolicy`, :130 `TokenScopePolicy`, :157 `OwnershipPolicy`; RoleLevel canonical at fe-policy/src/role_level.rs:8 |
| fe-hexon gap closed through the engine | complete | fe-hexon/src/authz.rs:26 `authorize`, :84-152 `publish_as`/`install_as`/`uninstall_as`/`list_installed_as`/`search_local_as`; tests :411, :477 |
| No hardcoded role-name list / ad-hoc compare outside the engine | complete (adapters kept) | fe-database/src/rbac.rs:5 imports engine, :51 `evaluate_write`, :84 `require_write_role` delegates; the only role-name array left is a test at rbac.rs:159. fe-api `require_*` remain as thin adapters into the engine (fe-api/src/auth.rs:236-256; `api_token_context` :213; test :290) |
| P2P write path gated | complete (by supersession) | admission moved to the DB thread: fe-database/src/handlers/replicated_row.rs:655 `admit_inbound_row`, engine call :882; fe-sync/src/write_policy.rs:10-20 `PolicyHandle` kept, no production caller |
| fe-plugin and fe-webview adapters | complete | fe-plugin/src/host_env.rs:165 engine, :169 `require`; capability.rs:224 `to_auth_context`; fe-webview/src/petal_portal.rs:81 `tab_engine`, :103 `can_view_config` |
| Amendment: causal-DAG membership + strong-removal resolver | obsolete (scoper) | superseded by canonical_data_log_20260808 (fe-canonical-log signing/capability/author_key; fe-database/src/canon_log/) |
| Open questions 1-3 | answered by code | Anonymous is a variant (types.rs:13) mapping to RoleLevel::None (:44); engine lives in the new fe-policy crate (Cargo.toml:24); AuthContext is pre-fetched, evaluate does no I/O |

## Plan tasks

| Task | Status | Evidence / class |
|---|---|---|
| 1.1-1.6 engine core | complete | rows above |
| 2.1 rbac adapter | complete | rbac.rs:51, :84 |
| 2.2 fe-hexon | complete | authz.rs:26-152 |
| 2.3 fe-sync gate | complete | write_policy.rs:10-36 |
| 2.4 fe-plugin, 2.5 fe-webview | complete | host_env.rs:165-169; petal_portal.rs:81-111 |
| 3.1 fe-api auth.rs through the engine | complete | auth.rs:236-256; fe-api/src/hexon.rs:219-238 |
| 3.2 hexon-registry routes via `*_as` | complete (by supersession) | hexon.rs:315, :368 call `require_hexon_petal_access` |
| 3.3 peer roles, strict gate | complete (by supersession) | replicated_row.rs:655, :882 |
| 3.4 TokenScope + Ownership | complete | policies.rs:130, :157; fe-api/tests/api_integration.rs:456; authz.rs:41 |
| 3.5 causal-DAG membership | obsolete (scoper) | ticked with SUPERSEDED prefix by T-DATA (f0aea31f) |

- Gate hand-offs: no R-gate reviewed this track this round; nothing to route.
- SEC-INBOX: entry 4 (fe-policy/src/role_grants.rs, grant windows) sits in fe-policy but belongs to group CLOCK (R-CLOCK-1); not a requirement of this track, not handled here.
- §9: no question concerns this track.

## HANDED-OFF (owner = next round)

1. HANDED-OFF (T-DATA, next round) - metadata.json status is still `in_progress` with every plan task ticked; set it to complete (or record why it stays open) in the next bookkeeping pass.

Verdict: COMPLETE — every requirement and task is met at HEAD or obsolete by supersession; only the metadata status lags.
