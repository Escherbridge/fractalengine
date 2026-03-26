# Crate Dependency Graph

Internal workspace crate dependencies.

```mermaid
graph TD
    FE[fractalengine<br/>Binary Entry Point]

    FE --> Runtime[fe-runtime<br/>Bevy App + Channels]
    FE --> Network[fe-network<br/>libp2p + iroh]
    FE --> Database[fe-database<br/>SurrealDB]

    Runtime --> Network
    Runtime --> Database
    Runtime --> Renderer[fe-renderer<br/>Asset Pipeline]
    Runtime --> WebView[fe-webview<br/>wry Browser]
    Runtime --> UIPlugin[fe-ui<br/>egui Panels]
    Runtime --> Sync[fe-sync<br/>Replication]

    Network --> Identity[fe-identity<br/>Ed25519 + JWT + DID]
    Database --> Identity
    Auth[fe-auth<br/>Session + Handshake] --> Identity

    Network --> Auth
    Database --> Auth

    Renderer --> Identity
    Sync --> Network
    Sync --> Database

    WebView --> Auth

    classDef binary fill:#e74c3c,color:#fff
    classDef core fill:#3498db,color:#fff
    classDef infra fill:#2ecc71,color:#fff
    classDef feature fill:#f39c12,color:#fff

    class FE binary
    class Runtime,Network,Database core
    class Identity,Auth infra
    class Renderer,WebView,UIPlugin,Sync feature
```

## External Dependency Map

```mermaid
graph LR
    subgraph Engine["3D Engine"]
        Bevy[bevy 0.18]
        Egui[bevy_egui 0.39]
    end

    subgraph P2P["Peer-to-Peer"]
        Libp2p[libp2p 0.56]
        IrohB[iroh-blobs 0.35]
        IrohG[iroh-gossip 0.35]
        IrohD[iroh-docs 0.35]
    end

    subgraph Data["Data Layer"]
        Surreal[surrealdb 3.0]
        Serde[serde 1.x]
        SerdeJ[serde_json 1.x]
    end

    subgraph Crypto["Cryptography"]
        Ed25519[ed25519-dalek 2.2]
        JWT[jsonwebtoken 10.3]
        Blake[blake3 1.x]
        Keyring_[keyring 3.x]
    end

    subgraph Async["Async Runtime"]
        Tokio_[tokio 1.x]
        Crossbeam_[crossbeam 0.8]
    end

    subgraph Util["Utilities"]
        Tracing_[tracing 0.1]
        ULID[ulid 1.x]
        Anyhow[anyhow 1.x]
        Thiserror_[thiserror 1.x]
    end
```
