---
type: Review
gate: R-UI-3
track: inspector_settings_20260419
reviews: [UI-07, UI-08]
date: 2026-11-13
security_review: true
---

# R-UI-3: Access tab (UI-07 grant record, UI-08 presets + EXT-18)

Adversarial pass over `3e11e58e` (UI-07) and `6c4735a7` (UI-08) against `git diff 7af0a144`. Security review done (authz, trust boundary, bounds, fail-closed, secrets). Security inbox: no entry names an owned file.

R-UI-3-01 FIXED - fe-ui/src/dialogs/entity_access.rs:96 - Managers only queued every peer with a below-manager grant from ANY grantor; a non-owner Manager's Revoke only withdraws its own statement (fe-database/src/role_manager.rs:313-337), so it sent no-op revokes and the confirm overstated "revoke N grants". Now: Owner = strongest live grant viewer/editor (owner revoke blocks the cell); Manager = only its own viewer/editor grants; never the local DID or an effective owner.
R-UI-3-02 FIXED - fe-ui/src/dialogs/entity_settings.rs:387 - Managers only sent SetVerseDefaultAccess without the `can_write` gate the Public/Members buttons use, and started the revoke queue even when that send failed (half-applied preset). Default part now needs `can_write`; revokes start only if it went out.
R-UI-3-03 FIXED - fe-ui/src/dialogs/entity_access.rs:413 - Managers only was enabled at verse scope with nothing to do (no write right, zero revokes), and its confirm read "revoke 0 grants" while also changing the verse default. Enabled only when it sets the default or has revokes; confirm names the default change ("Confirm: default none, revoke N grants").
R-UI-3-04 FIXED - fe-ui/src/dialogs/entity_access.rs:934 - the freshly minted token still showed the raw `Expires: <rfc3339>`; now `token_expiry_label` with the raw value on hover, matching the token list.
R-UI-3-05 FIXED - fe-ui/src/dialogs/entity_access.rs (role_confirmation_refresh_journey tail) - test gap for 01: journey extended (no new fn) with a Manager-scoped run: another grantor's viewer grant is not queued, exactly one Revoke for the Manager's own grant, then the read-back; journey ends with no request in flight so the caller's invitation steps still run.
R-UI-3-06 HANDED-OFF (I-UI-1) - fe-ui/src/dialogs/entity_access.rs:169 - EXT-18 is not wired: `mark_verse_local_only` has no caller, so the local-only reason and the disabled Generate invite never appear. NEEDS fe-ui/src/panels/mod.rs:401 (dialog_entity_settings closure), before render: `crate::dialogs::entity_access::mark_verse_local_only(&mut ui_mgr.active_dialog, |verse_id| hierarchy.verses.iter().find(|v| v.id == verse_id).map(|v| v.namespace_id.is_none()));`
R-UI-3-07 HANDED-OFF (I-BATCH-1) - conductor/test-batches.md:211 (R6 fe-ui line) - no filter reaches `actions::tests::active_dialog_entity_settings_opens_and_closes`, which drives the whole UI-07/UI-08 journey; append `actions::tests::active_dialog_entity_settings` to the R6 `cargo test -p fe-ui --lib --` filter.
R-UI-3-08 HANDED-OFF (I-DOC-UI) - fe-ui/src/dialogs/AGENTS.md - no 'access record' or 'presets' section: needs what Granted here / Inherited / Blocked here mean, that the record is the latest statement per (peer, grantor) not a log, the three presets, why Managers only is sequential behind one confirm and owner-vs-manager revoke reach, and the local-only verse reason.
R-UI-3-09 HANDED-OFF (CLOCK-04 owner, next round) - fe-database/src/role_grants.rs:484 - `load_scope_statements` is `SELECT * FROM role_grant WHERE scope = $s` with no LIMIT; `grant_record` truncates to 256 only after loading all rows (rest of `load` is LIMIT 257 + 1 MiB capped). Low: admitted statements need grantor authority. Add `ORDER BY hlc DESC LIMIT 257` (or a bounded variant) to the seam.
R-UI-3-10 KEPT (matches the existing radio path entity_settings.rs:194; set_default_access re-checks Manager at verse scope) - fe-ui/src/dialogs/entity_settings.rs:394 - default_access buffer updated after a successful send, before the DB confirms.
R-UI-3-11 KEPT (dialog render, not a per-frame hot system; bounded to 256 peers) - fe-ui/src/dialogs/entity_access.rs:472 - `peer_label` allocates per row in `sort_by_cached_key`.
R-UI-3-12 KEPT (authority() runs before grant_record; snapshot fields bounded_identity-filtered and capped; Debug prints only grant_count; fail-closed on read error; grants come only from the admitted role_grant table whose windows place_statement re-derives) - fe-database/src/handlers/entity_access.rs:189 - security review of the grant record: no authz, bound, secret-logging or forgery defect found.
R-UI-3-13 KEPT (server authoritative: each Revoke goes through authority() and revoke_role_checked; UI never writes roles optimistically; one request in flight, queue cleared on refusal, timeout, target switch and refresh) - fe-ui/src/dialogs/entity_access.rs:251 - sequential bulk revoke discipline verified.

Verdict: PASS-WITH-FIXES
