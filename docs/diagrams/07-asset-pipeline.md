# Asset Pipeline

## GLTF Ingestion Flow

```mermaid
graph TD
    Upload[Operator Uploads<br/>GLB File] --> Validate{Valid GLTF/GLB?}
    Validate -->|No| Reject[Reject Upload<br/>GLTF/GLB only]
    Validate -->|Yes| Size{Size ≤ 256 MB?}
    Size -->|No| Reject2[Reject Upload<br/>Exceeds limit]
    Size -->|Yes| Hash[BLAKE3 Hash<br/>Content Address]

    Hash --> Ingest[AssetIngester::ingest]
    Ingest --> Store[Store in local<br/>asset cache]
    Store --> DB[Record in SurrealDB<br/>model table]
    DB --> Publish[Publish hash via<br/>iroh-blobs]

    subgraph "Content Addressing"
        Hash2[BLAKE3(glb_bytes)]
        ID["Asset ID = hash<br/>e.g. bafk...abc123"]
    end

    Hash --> Hash2 --> ID
```

## Asset Distribution (Peer-to-Peer)

```mermaid
sequenceDiagram
    participant Visitor as Visiting Peer
    participant Cache as Local Asset Cache
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

        Visitor->>Cache: Store (LRU, 2GB limit,<br/>7-day eviction)
    end

    Visitor->>Visitor: Bevy GLTF Loader<br/>Spawn 3D model
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
