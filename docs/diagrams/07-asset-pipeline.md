# Asset Pipeline

## GLTF Import Flow

```mermaid
graph TD
    Import[Right-click → Import GLB] --> FileDialog{File Dialog<br/>Select .glb/.gltf}
    FileDialog -->|Selected| Read[Read file bytes]
    Read --> Hash[BLAKE3 Hash<br/>Content Address]

    Hash --> BlobStore[BlobStore::add_blob<br/>Content-addressed storage]
    Hash --> Copy[Copy to<br/>assets/imported/{hash}.glb]

    BlobStore --> AssetRow[CREATE asset {<br/>  asset_id, name,<br/>  content_type, size_bytes,<br/>  content_hash<br/>}]
    Copy --> NodeRow[CREATE node {<br/>  node_id, petal_id,<br/>  asset_id, position,<br/>  elevation<br/>}]
    AssetRow --> NodeRow

    NodeRow --> Result[DbResult::GltfImported]
    Result --> Spawn[Bevy: Spawn SceneRoot<br/>at click position]
    Result --> Sidebar[Update sidebar<br/>hierarchy tree]

    subgraph "Content Addressing"
        Hash2[BLAKE3 bytes → hex string]
        Path["Asset path: blob://{hash}.glb<br/>or assets/imported/{hash}.glb"]
    end

    Hash --> Hash2 --> Path
```

## Legacy Base64 Migration (Phase C)

```mermaid
sequenceDiagram
    participant DB as SurrealDB (T3)
    participant Blob as BlobStore
    participant Startup as DB Thread Startup

    Note over Startup: Runs eagerly at startup

    Startup->>DB: SELECT asset_id, data, content_hash<br/>FROM asset WHERE data != NONE

    loop Each legacy asset
        DB-->>Startup: { asset_id, data: "base64...", content_hash: null }
        Startup->>Startup: Base64 decode → raw bytes
        Startup->>Startup: BLAKE3(raw_bytes) → hash
        Startup->>Blob: add_blob(raw_bytes)
        Startup->>DB: UPDATE asset SET<br/>  content_hash = hash,<br/>  data = NONE
    end

    Note over Startup: Legacy rows now reference<br/>blob store via content_hash
```

## Asset Distribution (Peer-to-Peer)

```mermaid
sequenceDiagram
    participant Visitor as Visiting Peer
    participant Cache as Local BlobStore
    participant Host as Host Node
    participant Blobs as iroh-blobs

    Visitor->>Cache: Lookup asset by BLAKE3 hash
    alt Cache Hit
        Cache-->>Visitor: Cached GLB bytes
        Note over Visitor: Skip network fetch
    else Cache Miss
        Visitor->>Blobs: Request hash from peers
        Blobs->>Host: GET content (BLAKE3 verified)
        Host-->>Blobs: GLB bytes
        Blobs-->>Visitor: Verified bytes
        Note over Blobs: Automatic integrity check:<br/>BLAKE3(received) == requested hash

        Visitor->>Cache: Store in BlobStore
    end

    Visitor->>Visitor: Bevy AssetServer loads<br/>blob://{hash}.glb#Scene0
```

## Dead-Reckoning (Bandwidth Reduction)

```mermaid
sequenceDiagram
    participant Host as Host Node
    participant Net as Network (iroh-gossip)
    participant Visitor as Visiting Peer
    participant DR as DeadReckoningPredictor

    Host->>Net: Transform update {pos, vel, accel}
    Net-->>Visitor: Received
    Visitor->>DR: Set base state

    loop Every Frame
        DR->>DR: Predict position from<br/>velocity + acceleration
        DR->>Visitor: Render at predicted position
    end

    Note over Host,Visitor: Network update only when<br/>prediction error > threshold

    Host->>Net: Correction {pos, vel, accel}
    Net-->>Visitor: Received
    Visitor->>DR: Snap to corrected state
    Note over DR: 60–90% bandwidth reduction
```
