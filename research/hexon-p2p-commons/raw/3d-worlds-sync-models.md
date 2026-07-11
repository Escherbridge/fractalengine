# Raw: Distributed 3D worlds sync models

Fetched: 2026-07-11
Intent: Graveyard analysis of sync architectures — what compromise each made
Sources: WebSearch aggregations (Croquet/Multisynq, Decentraland Catalyst, Third Room, Mozilla Hubs, Resonite/NeosVR)

## Croquet / Multisynq (reflector model)
- Multiplayer code executes on EACH client in a synchronized deterministic VM ("Teatime"). Given same initial state + same event sequence, every device produces bit-identical result.
- A CENTRALIZED reflector issues periodic ticks (~25-50 ms) — a synchronized global shared clock. Reflector timestamps all external messages from views and replicates them back to all peers of a session.
- Reflectors are "extremely efficient" because they do NO computation on app data — they only order events into a single canonical stream + provide a global heartbeat.
- PATTERN: computation is decentralized (client VMs) but the ORDERING AUTHORITY (reflector) is centralized. It's a "client-side multiplayer" that still needs a central sequencer.
- Sources: https://en.wikipedia.org/wiki/Croquet_Project ; https://docs.multisynq.io/essentials/sync ; https://github.com/croquet/croquet

## Decentraland (catalyst servers)
- A Catalyst bundles services = backbone; runs "decentralized storage for most content" + orchestrates peer comms.
- Content Server stores Entities (scenes, wearables, profiles); provides distributed file-system; clients query indices, retrieve files, deploy entities, download periodic snapshots.
- Content Servers auto-sync with each other "as long as they were all approved by the DAO." Fully-meshed among APPROVED nodes.
- PATTERN: "decentralized" but the node set is GATED by DAO approval — permissioned federation, not open P2P. Discovery + content hosting sits on a curated server tier.
- Sources: https://github.com/decentraland/catalyst ; https://docs.decentraland.org/contributor/architecture/catalyst

## Third Room (Matrix)
- Decentralized metaverse on Matrix protocol, governed by Matrix.org Foundation. Adds 3D world hosting on Matrix.
- STATUS: team laid off, project on pause/archived (Apache 2.0, code remains). Mastodon announcement ~mid-2023.
- PATTERN: died from FUNDING/org restructuring, not technical failure. Depended on a foundation's continued investment. "Decentralized" protocol still needed a centrally-funded team to build the client.
- Sources: https://github.com/matrix-org/thirdroom ; https://mastodon.matrix.org/@thirdroom/110787604385806102

## Mozilla Hubs
- Shut down 2024-02-13 (org-wide restructuring, shift to AI). Support ended 2024-05-31; Mozilla closed the hosted AWS infra.
- Open-source "Hubs Community Edition" continues; runs on any Kubernetes platform (AWS/Azure/GCP). Managed by Hubs Foundation.
- PATTERN: the HOSTED service (the part everyone used) died; the self-hostable code survived but requires each operator to run Kubernetes infra. Continuity ≠ usability — self-hosting bar is high.
- Sources: https://support.mozilla.org/en-US/kb/end-support-mozilla-hubs ; https://www.roadtovr.com/mozilla-hubs-shutdown-web-xr/

## Resonite / NeosVR (host-authoritative)
- Data model sync: local change → shared with HOST → host forwards to rest of session. Host-authoritative.
- Contention: version number metadata on every update; host decides authoritative value. Same version from 2 users → FIRST kept, later discarded (LWW-ish, host-arbitrated).
- Session: whoever starts a world becomes HOST; others connect as clients. Binary protocol assumes identical code versions.
- PATTERN: within a session there IS a center — the host. "Eventual consistency" but through a single authoritative host, not a leaderless CRDT. When host leaves, session needs migration or dies.
- Sources: https://wiki.resonite.com/Data_model_synchronization ; https://wiki.resonite.com/Architecture_Overview
