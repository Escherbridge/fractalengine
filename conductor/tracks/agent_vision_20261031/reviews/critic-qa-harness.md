---
type: Review
date: 2026-11-01
status: provisional-pending-integrated-verification
reviewer: Codex
---

# QA harness critic: security and lifecycle

VERDICT: PROVISIONAL — safety fixes applied; Rust compilation and combined
post-fix live smoke remain pending. This is not a final PASS.

The independent static review found and fixed five issues:

| Severity | Finding | Applied correction |
| --- | --- | --- |
| HIGH | Example-only Cargo selection could stage a stale GUI under current HEAD | Explicit GUI build and installer build; unique immutable stage; executable SHA-256 and tracked/untracked build-input fingerprint |
| HIGH | Persistent PID-only stop could kill a recycled unrelated PID | PID + executable + creation ticks; held process handles; Toolhelp descendants checked and stopped before root |
| HIGH | QA used real user settings and OS-keystore identity | Explicit run-local storage root; isolated settings/node key; memory secret store; force local blob/P2P/hexons; skip user blob migration |
| MEDIUM | Windows secret creation applied permissions and reopened by pathname | Owner DACL supplied at CreateFileW creation; one nonshared handle; ancestor handles reject and pin reparse boundaries |
| MEDIUM | Cleaner lacked path/reparse confinement and live-stage protection | Confined literal paths; retain trees containing reparse points; retain legacy/identity-verified live runs and referenced stages |

Startup now has enclosing failure cleanup until a complete run record is
handed off. Asset fixtures are copied into the run, replacing the writable
repository junction. Pass instructions save the owned run directory and pass
it explicitly to stop, avoiding mutable discovery pointers during cleanup.

Existing Windows secure-file and settings-restart journeys were extended;
no test functions were added. The settings path journey uses pure inputs,
without global environment mutation. Windows file coverage includes atomic
replacement, refused overwrite, and rejected junction parents.

Evidence collected in this pass:

- PowerShell parser accepted every tools/qa script.
- Shared process helper accepted its actual current-process identity and
  refused a deliberately stale creation time, without terminating processes.
- Native Toolhelp snapshot executed successfully without WMI/CIM access.
- Source fingerprint completed and returned 64 hexadecimal characters.
- Targeted rustfmt parsed/formatted the three touched Rust files.
- git diff --check passed.
- Windows API declarations were compared with vendored windows-sys 0.61.2
  CreateFileW and SECURITY_ATTRIBUTES definitions.

No application was launched in this pass, and no Cargo build/check/test or
Clippy sweep was run. Per campaign policy the combined build and smoke run
follows the pending navigation/input/surface fixes; the integrated Rust test,
lint and formatting sweep runs once after the campaign changes.

Remaining verification must exercise fresh stage/launch, active-verse camera
and screenshot, copied assets and run-local state, clean exact-process stop,
failed startup cleanup, and the extended journey tests. Legacy stages without
stage.json intentionally cannot be launched by the strengthened scripts.
The already-staged pre-fix executable does not verify these changes.

The cleaner conservatively rejects existing reparse trees; it does not claim
security against a concurrently malicious shared filesystem writer. Unix
secret creation rejects existing symlink ancestry and retains create_new/0600,
but only Windows pins ancestors against write/delete substitution. These
limits are documented rather than treated as tested cross-platform claims.
