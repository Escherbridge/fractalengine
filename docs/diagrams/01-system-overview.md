# System Overview

High-level view of FractalEngine's runtime architecture: the seven-thread topology with all major subsystems. The headless `fe-relay` runs T2 through T6 and starts the plugin host (T7) only when built with the opt-in `plugin-host` feature (`fractalengine-relay/src/main.rs:355-359`).

```mermaid
graph TB
    subgraph T1["T1 - Main Thread (OS)"]
        Bevy["Bevy ECS Scheduler"]
        Wry["wry / Tauri WebView<br/>Event Loop"]
        UI["bevy_egui<br/>Admin Panels"]
        Renderer["Asset Pipeline<br/>GLTF Loader"]
        Pool["Bevy AsyncComputeTaskPool<br/>(inside T1)"]
        Drain["Event Drain Systems<br/>drain_network / drain_db"]
    end

    subgraph T2["T2 - Network Thread"]
        Tokio2["Dedicated Tokio Runtime"]
        Libp2p["libp2p Swarm: Kademlia + QUIC"]
    end

    subgraph T3["T3 - Database Thread"]
        Tokio3["Dedicated Tokio Runtime"]
        Surreal["SurrealDB 3<br/>SurrealKV Backend"]
        RBAC["RBAC (fe-policy, deny-by-default)"]
        OpLog["Immutable Op-Log<br/>Lamport Clock + Signatures"]
        Schema["Schema: Verse, Fractal, Petal,<br/>Node, Asset, Room, Model,<br/>Role, VerseMember, OpLog"]
    end

    subgraph T4["T4 - Sync Thread"]
        Blobs["iroh-blobs<br/>Asset Distribution"]
        Gossip["iroh-gossip<br/>Zone Events"]
        Docs["iroh-docs<br/>CRDT Sync"]
    end

    subgraph T5["T5 - Replication Bridge"]
        Bridge["OwnedService 'replication'<br/>try_send + drop counter"]
    end

    subgraph T6["T6 - API Gateway"]
        Api["axum HTTP/WS on 127.0.0.1:8765<br/>+ MCP tools"]
    end

    subgraph T7["T7 - Plugin Host"]
        Plugins["Rhai + WASM sandboxes"]
    end

    Bevy -->|NetworkCommand| Ch1["crossbeam<br/>bounded 256"]
    Ch1 --> Libp2p
    Libp2p -->|NetworkEvent| Ch2["crossbeam<br/>bounded 256"]
    Ch2 --> Drain

    Bevy -->|DbCommand| Ch3["crossbeam<br/>bounded 256"]
    Ch3 --> Surreal
    Surreal -->|DbResult| Ch4["crossbeam<br/>bounded 256"]
    Ch4 --> Drain

    Surreal -->|row events| Bridge
    Bridge -->|SyncCommand| Blobs
    Bridge -->|SyncCommand| Docs

    Api -->|DbCommand channel| Ch3
    Plugins -->|scoped bridge| Surreal

    Bevy --> Pool
    Drain --> Bevy
```

Sources: see `docs/diagrams.md` section "diagram-truth".
