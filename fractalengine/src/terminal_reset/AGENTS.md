# Terminal maintenance supervisor

## ?terminal-reset

The lifecycle contract and startup wiring rationale live in `../AGENTS.md` ?terminal-reset.

- `mod.rs` owns the monotonic state machine, exact request, service ownership, reset proof and final transaction/join outcome.
- `observation.rs` drains bounded old channels without normal UI/API projection. Draining remains active through stop/join, including failures, so bounded result queues cannot pin workers.
- `presentation.rs` renders only maintenance status and terminal Close controls. It cannot restart services or repeat reset.

Normal Update and EguiPrimaryContextPass schedules are dropped at a First boundary; their locals cannot retain a queued mutation worker. PostUpdate/Last platform rendering and cleanup remain installed. A matching receipt and DB join are separate facts. Unconfirmed outcomes keep the process terminal; later receipts may clarify data without resuming producers.

The host also replaces PendingPathEdits, PendingGpxImports, TrackRouteMap and
TrackStyleMap after invalidating the UI managers. This clears the admitted-current
projection backlog and parsed route data, rather than retaining inert old scene
queues until terminal Close. VerseManager replacement clears all private current
reads, source identities and recovery intents.

Host GPX/path requests already accepted by the DB retain their outcome-only
owners and source payloads during freeze. The terminal Update cannot continue
those workflows. Their resources are replaced only after the database worker
has actually joined; a quiescence or join failure leaves those owners retained
in the terminal process. This is separate from clearing pure read/render state.
