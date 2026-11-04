---
type: Review
status: source_reviewed_live_pending
---

# Owner QA/provider handoff ordering: independent source review

Reviewer `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the exact four-path source correction.** The explicit ordering resolves the inspected shared-output race without changing authority or native suppression. No schedule-cycle or remaining source blocker found. This is source approval; matching live recovery and compilation remain separate gates.

## Exact reviewed delta

Main HEAD `5a9eeb934ef27e8319f5d9aaab5038d19a6ab151`. Allowlisted `git diff --binary HEAD` is **2845 bytes**, SHA-256 **`212a3f50ab1f2bf0695dcaa102b13c38eb97958009dbda8073b2bab2700e54a9`**.

| Path | Current raw SHA-256 |
|---|---|
| `fractalengine/src/native_window_witness/mod.rs` | `2449813abd6c6b21faa79c19da0d2fd9de031c94fcdc23ef0ea91ddc6874964e` |
| `fractalengine/src/native_window_witness/provider_output.rs` | `174bb0397eda4967a93821cfcf9f2338423e66c16c450fe70efd339a29a60208` |
| `fractalengine/src/qa_debug_bridge.rs` | `74613e8d87584fa6643bfafc3f4155518d7ffa5a390f32a46e2e84523524fb6e` |
| `fractalengine/src/native_window_witness/AGENTS.md` | `27fb73f78f06f793c0a060f683382c8b9ea407e1ad3a3446b510c05c91baea08` |

The complete source changes are a crate-visible derived ProviderHandoff SystemSet declaration, actual native prepare.in_set(ProviderHandoff), actual Owner serve.before(ProviderHandoff), and the directory-level ordering explanation. No function body, request/response schema, data model or permission predicate changed. Allowlisted read-only diff check passes. Other dirty findings/attributes/Conductor/session-log files and QA evidence are outside this source approval; the index was empty at inspection.

## Actual schedule and shared output

The resulting order is source-supported:

1. EgUI ProcessOutput completes this frame's EguiOutput.
2. Existing Owner QA serve, explicitly after ProcessOutput, runs before the actual prepare system through ProviderHandoff.
3. Native prepare remains in PostProcessOutput and before annotation and the named bevy_egui accessibility forwarder; it remaps or suppresses that same actual output under its existing checks.
4. Native forwarder and alternate-primary-publisher hold retain their reviewed edges; WindowChromeApply remains after PostProcessOutput, with Owner serve still explicitly before it. Native intake remains after that completed provider/window phase.

No inverse edge puts serve after ProviderHandoff or requires prepare before ProcessOutput. The set membership makes the dependency act on the real production prepare registration, not a new helper mirror. Both systems already share conflicting Context/output access; the added edge supplies the previously absent deterministic direction. The set is crate-visible in the existing host module, not a public control or privilege interface.

Owner reads now observe the current completed **physical-ID** tree before native aliases or output suppression. Existing response/layout IDs therefore remain in the same identity space for ordinary QA observations. There is no retained snapshot cache, inferred remainder, alternate native provider tree or retroactive Last change. If ProcessOutput has no tree, Owner still receives its existing readiness refusal; the ordering does not fabricate one.

The actual Windows-bound output is still independently validated/remapped/suppressed afterward, including inactive/missing exact native ownership and unsafe full-tree failures. Checked failure retirement, original one-second proof, accepted-read ownership, alias tombstones, HOLD, actual root/property publication and alternate-publisher suppression are byte-unchanged. Owner observation does not become native provider/navigation/capture evidence.

## Authority and state gates unchanged

Owner QA enablement, role floor, loopback/input opt-in, exact active view scope, FrameNavigation currentness, primary-context checks, password/text redaction, bounded flatten traversal and command/request handling are unchanged. Native AccessibilityRequested is not forced, and adapter/context/handler matching is not weakened. No fake role, Viewer proof, native action, input injection or UI side-effect body is added. The provider's read-only metadata rules and later operator/capture requirements remain.

QA40's repeated409 receipts remain failed evidence. Parent reports its exact owned stop and eight identities absent at19:58:49Z; this source review did not independently inspect process state. A later successful UI read must be attached to its own matching source/stage, not used to rewrite those original outcomes.

## Verification scope

No blocking need for a trivial duplicate-set-order test was found. Such a fixture would restate the intended edges without traversing real Owner/native production branches. For this narrow reversible registration change, root's proposed **matching live UI recovery** is the meaningful immediate verification: fresh stage/source; ordinary Owner UiState, UiTree and screenshot without native-client activation; nonempty named physical-ID tree with response/layout joins and normal guarded input; preserve the current native suppression/positive-proof boundaries and normal role/scope/privacy refusals. Existing full native journeys remain required in the final integrated sweep.

If a later automated regression harness is authored, it should install the actual Owner bridge and native provider registrations with a real completed EguiOutput, a valid Owner request, and inactive/absent native ownership; assert Owner response sees the completed physical tree while the actual downstream native output is subsequently suppressed. A successful native remap variant should observe physical IDs first and aliases only at the later provider boundary. That is useful actual registration coverage, but is not required as an unexecuted mirror before source approval of this four-path fix.

Only this review artifact was authored. No production/index/history/configuration mutation, compiler/build/formatter/test, app, OS/process/input/capture, API/DB/store or network operation occurred.
