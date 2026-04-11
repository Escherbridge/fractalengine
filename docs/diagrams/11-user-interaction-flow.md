# User Interaction Flow

## End-to-End: Create Verse → Fractal → Petal

```mermaid
sequenceDiagram
    participant Op as Node Operator
    participant UI as egui Sidebar
    participant T1 as Bevy System (T1)
    participant T3 as SurrealDB (T3)

    Op->>UI: Click "+" → Create Verse
    UI->>T1: DbCommand::CreateVerse { name: "My World" }
    T1->>T3: Process command
    T3->>T3: INSERT INTO verse { verse_id, name, created_by, namespace_id }
    T3-->>T1: DbResult::VerseCreated { id, name }
    T1->>UI: Show verse in hierarchy tree

    Op->>UI: Click "+" on Verse → Create Fractal
    UI->>T1: DbCommand::CreateFractal { verse_id, name: "District A" }
    T1->>T3: Process command
    T3->>T3: INSERT INTO fractal { fractal_id, verse_id, owner_did, name }
    T3-->>T1: DbResult::FractalCreated { id, verse_id, name }

    Op->>UI: Click "+" on Fractal → Create Petal
    UI->>T1: DbCommand::CreatePetal { fractal_id, name: "Showroom" }
    T1->>T3: Process command
    T3->>T3: INSERT INTO petal { petal_id, fractal_id, name }
    T3-->>T1: DbResult::PetalCreated { id, fractal_id, name }
    T1->>UI: Show full tree: Verse → Fractal → Petal
```

## End-to-End: Import GLB Model

```mermaid
sequenceDiagram
    participant Op as Node Operator
    participant UI as egui Viewport
    participant T1 as Bevy System (T1)
    participant T3 as SurrealDB (T3)
    participant Blob as BlobStore
    participant Bevy as Bevy AssetServer

    Op->>UI: Right-click in viewport
    UI->>UI: Show context menu at world position
    Op->>UI: Click "Import GLB..."
    UI->>UI: Open file dialog → select duck.glb

    UI->>T1: DbCommand::ImportGltf { petal_id, name, file_path, position }
    T1->>T3: Process command

    T3->>T3: Read file bytes
    T3->>T3: BLAKE3(bytes) → content_hash
    T3->>Blob: add_blob(bytes)
    T3->>T3: Copy to assets/imported/{hash}.glb
    T3->>T3: CREATE asset { asset_id, content_hash, size_bytes }
    T3->>T3: CREATE node { node_id, petal_id, asset_id, position }

    T3-->>T1: DbResult::GltfImported { node_id, asset_path, position }

    T1->>Bevy: asset_server.load("{asset_path}#Scene0")
    T1->>T1: Spawn SceneRoot + Transform at position
    T1->>UI: Add node to sidebar hierarchy
```

## End-to-End: Verse Invite Flow

```mermaid
sequenceDiagram
    participant Alice as Alice (Host)
    participant AliceUI as Alice's UI
    participant T3A as Alice's DB (T3)
    participant Bob as Bob (Joiner)
    participant BobUI as Bob's UI
    participant T3B as Bob's DB (T3)

    Alice->>AliceUI: Click "Invite" on Verse
    AliceUI->>T3A: DbCommand::GenerateVerseInvite { verse_id, include_write_cap, expiry_hours }
    T3A->>T3A: Sign invite with Ed25519 keypair
    T3A-->>AliceUI: DbResult::VerseInviteGenerated { invite_string }
    AliceUI->>AliceUI: Show invite string in dialog (copyable)

    Alice-->>Bob: Share invite string (out-of-band)

    Bob->>BobUI: Click "Join" → Paste invite string
    BobUI->>T3B: DbCommand::JoinVerseByInvite { invite_string }
    T3B->>T3B: Decode + verify Ed25519 signature
    T3B->>T3B: CREATE verse { verse_id, name }
    T3B->>T3B: CREATE verse_member { peer_did, status: active }
    T3B-->>BobUI: DbResult::VerseJoined { verse_id, verse_name }
    BobUI->>BobUI: Refresh hierarchy → show joined Verse
```

## End-to-End: Database Reset

```mermaid
sequenceDiagram
    participant Op as Node Operator
    participant UI as egui Sidebar
    participant T1 as Bevy System (T1)
    participant T3 as SurrealDB (T3)

    Op->>UI: Click "Reset Database" (bottom of sidebar)
    UI->>T1: DbCommand::ResetDatabase
    T1->>T3: Process command

    T3->>T3: REMOVE TABLE for all 10 tables
    T3->>T3: Re-apply full schema (define_table! DDL)
    T3->>T3: Re-seed default data (Genesis Verse + Petal)
    T3-->>T1: DbResult::DatabaseReset { petal_name, rooms }

    T1->>T1: Clear in-memory hierarchy
    T1->>T3: DbCommand::LoadHierarchy
    T3-->>T1: DbResult::HierarchyLoaded { verses }
    T1->>UI: Render fresh hierarchy tree
```

## End-to-End: Startup & Hierarchy Load

```mermaid
sequenceDiagram
    participant Main as main()
    participant T1 as Bevy (T1)
    participant T3 as SurrealDB (T3)

    Main->>T3: Spawn DB thread
    T3->>T3: Open SurrealKV ("data/fractalengine.db")
    T3->>T3: apply_schema() — idempotent DDL
    T3->>T3: Migrate legacy base64 assets → blob store
    T3-->>T1: DbResult::Started

    T1->>T3: DbCommand::Seed
    T3->>T3: Seed Genesis Verse + Fractal + Petal (if empty)
    T3-->>T1: DbResult::Seeded { petal_name, rooms }

    T1->>T3: DbCommand::LoadHierarchy
    T3->>T3: SELECT verse → fractal → petal → node + asset
    T3-->>T1: DbResult::HierarchyLoaded { verses }

    T1->>T1: Populate sidebar tree
    T1->>T1: Spawn scene entities for active petal
```
