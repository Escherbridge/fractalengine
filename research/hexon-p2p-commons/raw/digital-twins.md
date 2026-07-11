# RAW: Digital Twin Consistency & Update-Rate Requirements

Fetch date: 2026-07-11
Intent: concrete numbers for twin update rates / staleness tolerance; why twins centralize.

## Real-time requirements case study (IIoT, Flexible Production System)
Source: https://pmc.ncbi.nlm.nih.gov/articles/PMC8704305/ (Sensors, "When Digital Twin Meets
Network Softwarization in the Industrial IoT")

CONCRETE NUMBERS:
- Platform refresh / cycle time: **2 seconds** — "the main data rate is one packet every 2 s."
- Measured end-to-end packet delay (best config, TSCH/TASA): **average 178 ms, median 148 ms**;
  "all packets arrive with a delay that does not exceed 2 s."
- Requirement class: **hard real-time, periodic/isochronous** — "it must be ensured that all
  communications arrive within the period. If a network message gets lost then the controller
  will be deprived of fresh input data." Freshness is the KPI (a "freshness indicator").
- Oversampling variants: 1 pkt / 2 s (single), 2 (duplication), 3 (triplication). TSCH+TASA hit
  **100% freshness without oversampling** → redundancy was pure energy waste there.

INTERPRETATION for hexon: industrial control twins have HARD sub-2s deadlines with freshness as
a correctness property. A churned P2P commons CANNOT meet hard real-time control loops — gossip
fan-out + NAT hole-punch + CRDT merge jitter is many hundreds of ms to seconds under good
conditions and unbounded under churn. => hexon's honest niche is **soft-real-time / eventually-
consistent observational twins** (dashboards, monitoring, digital-shadow) NOT closed-loop control.
The staleness tolerance must be seconds-to-minutes, explicitly documented. High-rate control stays
on a LAN / broker; hexon federates the SHADOW, not the CONTROL LOOP.

## Why twin platforms centralize
Sources: https://medium.com/globant/modeling-digital-twins-8b758dc4b4d6 ,
Azure Digital Twins / AWS IoT TwinMaker (search synthesis)

- Azure Digital Twins puts the twin "in the middle of all services to ingest and aggregate data
  sources" — an explicit **centralized hub / single source of truth**. Model language: **DTDL**
  (JSON-like), forming a queryable/navigable **twin graph**.
- AWS IoT TwinMaker: binds asset properties to **live telemetry**, builds 3D scenes; source of
  truth = AWS IoT data.
- Both use **graph-based relationship models** + **event-driven updates** + **navigable graph
  queries**. Centralization is chosen for: unified governance, strong APIs, aggregation across
  many sites, and a single queryable graph. The "source of truth" framing is the antithesis of a
  federated commons: a twin platform's value proposition IS being the one authoritative graph.
- NEGATIVE-SPACE FINDING: I found NO production digital-twin platform that is P2P/federated by
  design. The literature and vendors treat centralization as a feature (governance, single graph),
  not an accident. A federated twin commons is genuinely novel territory — closest analog is
  event-sourced twins (append-only event log → materialized view), which maps DIRECTLY onto
  hexon's append-only op-log/delta model. Event sourcing is the bridge: a twin as a replayable
  event log is inherently CRDT/gossip-friendly; a twin as a mutable central graph is not.
- KTWIN (arXiv 2408.01635) — serverless Kubernetes twin platform — still centralized (k8s cluster)
  but shows the event-driven decomposition trend. TwinRAN/TwinRAN (arXiv 2407.13340) — twins the
  5G RAN in Azure — again cloud-centralized.

## Event-sourced twin ↔ hexon op-log mapping (analysis)
- Event-sourced digital twin = append-only immutable event stream + derived materialized state.
  This is EXACTLY hexon's "append-only op-log, replayable, content-addressed" delta file.
- Property: given the same event log, every peer materializes the same twin state (determinism of
  a fold/reduce over events) — this is the one place the commons gets STRONG convergence for free,
  PROVIDED the op-log order is agreed (HLC timestamps + causal ordering). Non-commutative ops still
  need CRDT discipline; commutative/idempotent event appends converge trivially.
