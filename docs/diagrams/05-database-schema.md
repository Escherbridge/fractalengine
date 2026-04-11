# Database Schema

SurrealDB tables and their relationships. All tables use `SCHEMAFULL` mode and are defined via the `define_table!` macro in `fe-database/src/schema.rs`.

## Entity Relationships

```mermaid
erDiagram
    verse ||--o{ fractal : contains
    verse ||--o{ verse_member : "has members"
    fractal ||--o{ petal : contains
    petal ||--o{ room : contains
    petal ||--o{ node : contains
    petal ||--o{ role : "has roles"
    petal ||--o{ model : contains
    petal ||--o{ op_log : "mutations logged"
    node }o--o| asset : "references"

    verse {
        string verse_id PK "ULID"
        string name
        string created_by "Peer DID"
        string created_at
        string namespace_id "iroh-docs replica ID (optional)"
    }

    verse_member {
        string member_id PK "ULID"
        string verse_id FK
        string peer_did "Ed25519 DID:key"
        string status "active | revoked"
        string invited_by "Peer DID"
        string invite_sig "Ed25519 signature"
        string invite_timestamp
        string revoked_at "optional"
        string revoked_by "optional"
    }

    fractal {
        string fractal_id PK "ULID"
        string verse_id FK
        string owner_did "Peer DID"
        string name
        string description "optional"
        string created_at
    }

    petal {
        string petal_id PK "ULID"
        string name
        string node_id FK "Host Node DID"
        string created_at
        string description "optional"
        string visibility "public | private | unlisted"
        array tags "string[]"
        string fractal_id FK "optional"
        geometry bounds "polygon (optional)"
    }

    room {
        string petal_id FK
        string name
        string description "optional"
        object bounds "FLEXIBLE (optional)"
        object spawn_point "FLEXIBLE (optional)"
    }

    model {
        string petal_id FK
        string asset_id PK "ULID"
        object transform "FLEXIBLE"
        string display_name "optional"
        string description "optional"
        string external_url "optional"
        string config_url "optional"
        array tags "string[]"
        object metadata "FLEXIBLE (optional)"
    }

    node {
        string node_id PK "ULID"
        string petal_id FK
        string display_name "optional"
        string asset_id FK "optional"
        geometry position "GeoJSON Point (XZ plane)"
        float elevation "Y axis height"
        array rotation "quaternion [x,y,z,w]"
        array scale "[x,y,z]"
        bool interactive "default false"
        string created_at
    }

    asset {
        string asset_id PK "ULID"
        string name
        string content_type "e.g. model/gltf-binary"
        int size_bytes
        string data "legacy base64 (optional)"
        string created_at
        string content_hash "BLAKE3 hex (optional)"
    }

    role {
        string node_id FK "Peer DID"
        string petal_id FK
        string role "public | editor | owner"
    }

    op_log {
        int lamport_clock PK "Deterministic ordering"
        string node_id FK "Author DID"
        string op_type "CREATE | UPDATE | DELETE"
        object payload "FLEXIBLE JSON"
        string sig "ed25519 signature"
    }
```

## Full Hierarchy

```
Verse                        top-level container (P2P namespace)
  ├── VerseMember            peer membership + invite records
  └── Fractal                groups petals under a verse
        └── Petal            a 3D world/space
              ├── Room       a zone within a petal
              ├── Model      a placed 3D object (legacy)
              ├── Node       an interactive object (with optional Asset)
              │     └── Asset   GLTF/GLB metadata + blob store reference
              ├── Role       RBAC assignment (node ↔ petal)
              └── OpLog      append-only mutation log
```

## RBAC Permission Model

```mermaid
graph TD
    subgraph Roles["Role Hierarchy"]
        Public["public<br/>(hardcoded floor)"]
        Editor["editor<br/>(write access)"]
        Owner["owner<br/>(full control)"]
    end

    Public --> Editor --> Owner

    subgraph Scope["Permission Scope"]
        Petal_Scope["Petal-level<br/>(default)"]
        Room_Scope["Room-level<br/>(override)"]
        Model_Scope["Model-level<br/>(override)"]
    end

    subgraph Enforcement["Enforcement Layer"]
        SDB["SurrealDB<br/>SCHEMAFULL tables"]
        RBAC["rbac::require_write_role()"]
        Note["Bevy systems NEVER<br/>check roles directly"]
    end

    Roles --> Scope --> Enforcement
```

## Op-Log Write Flow

```mermaid
sequenceDiagram
    participant Sys as Bevy System (T1)
    participant Ch as db_cmd channel
    participant DB as SurrealDB (T3)
    participant Log as op_log table

    Sys->>Ch: DbCommand::CreateNode { petal_id, name, position }
    Ch->>DB: Receive command

    DB->>DB: Check RBAC permissions<br/>for session's role
    alt Authorized
        DB->>DB: INSERT INTO node { ... }
        DB->>Log: INSERT INTO op_log {<br/>  lamport_clock: next(),<br/>  node_id: session.did,<br/>  op_type: "CREATE",<br/>  payload: json,<br/>  sig: ed25519_sign(payload)<br/>}
        DB-->>Ch: DbResult::NodeCreated
    else Denied
        DB-->>Ch: DbResult::Error("RBAC: insufficient permissions")
    end

    Ch-->>Sys: drain_db_results() picks up result
```

## Asset Import Flow

```mermaid
sequenceDiagram
    participant UI as egui Context Menu
    participant T1 as Bevy System (T1)
    participant T3 as SurrealDB (T3)
    participant Blob as BlobStore

    UI->>T1: Right-click → Import GLB
    T1->>T3: DbCommand::ImportGltf { petal_id, name, file_path, position }

    T3->>T3: Read file bytes
    T3->>T3: BLAKE3 hash → content_hash
    T3->>Blob: blob_store.add_blob(bytes)
    T3->>T3: Copy to assets/imported/{content_hash}.glb

    T3->>T3: CREATE asset { asset_id, name, content_hash, ... }
    T3->>T3: CREATE node { node_id, petal_id, asset_id, position, ... }
    T3-->>T1: DbResult::GltfImported { node_id, asset_path, position }

    T1->>T1: Spawn SceneRoot at position<br/>(Bevy GLTF loader)
```
