# Terminal maintenance supervisor

## ?terminal-reset

The lifecycle contract and startup wiring rationale live in `../AGENTS.md` ?terminal-reset.

- `mod.rs` owns the monotonic state machine, exact request, service ownership, reset proof and final transaction/join outcome.
- `observation.rs` drains bounded old channels without normal UI/API projection. Draining remains active through stop/join, including failures, so bounded result queues cannot pin workers.
- `presentation.rs` renders only maintenance status and terminal Close controls. It cannot restart services or repeat reset.

Normal Update and EguiPrimaryContextPass schedules are dropped at a First boundary; their locals cannot retain a queued mutation worker. PostUpdate/Last platform rendering and cleanup remain installed. A matching receipt and DB join are separate facts. Unconfirmed outcomes keep the process terminal; later receipts may clarify data without resuming producers.
