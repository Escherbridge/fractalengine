# Channel Bridge

All typed crossbeam channels that connect the three threads. Every channel is bounded at 256 to provide backpressure.

```mermaid
graph LR
    subgraph T1["T1 — Bevy Main"]
        NetCmdTx[NetworkCommandSender]
        NetEvtRx[NetworkEventReceiver]
        DbCmdTx[DbCommandSender]
        DbResRx[DbResultReceiver]
    end

    subgraph Channels["Crossbeam Channels (bounded 256)"]
        C1["net_cmd channel"]
        C2["net_evt channel"]
        C3["db_cmd channel"]
        C4["db_res channel"]
    end

    subgraph T2["T2 — Network"]
        NetCmdRx[Command Receiver]
        NetEvtTx[Event Sender]
    end

    subgraph T3["T3 — Database"]
        DbCmdRx[Command Receiver]
        DbResTx[Result Sender]
    end

    NetCmdTx -->|NetworkCommand| C1 --> NetCmdRx
    NetEvtTx -->|NetworkEvent| C2 --> NetEvtRx
    DbCmdTx -->|DbCommand| C3 --> DbCmdRx
    DbResTx -->|DbResult| C4 --> DbResRx
```

## Message Types

```mermaid
classDiagram
    class NetworkCommand {
        <<enum>>
        Ping
        Shutdown
    }

    class NetworkEvent {
        <<enum>>
        Pong
        Started
        Stopped
    }

    class DbCommand {
        <<enum>>
        Ping
        Seed
        Shutdown
        CreateVerse(name)
        CreateFractal(verse_id, name)
        CreatePetal(fractal_id, name)
        CreateNode(petal_id, name, position)
        ImportGltf(petal_id, name, file_path, position)
        LoadHierarchy
        GenerateVerseInvite(verse_id, include_write_cap, expiry_hours)
        JoinVerseByInvite(invite_string)
        ResetDatabase
    }

    class DbResult {
        <<enum>>
        Pong
        Seeded(petal_name, rooms)
        Started
        Stopped
        Error(String)
        VerseCreated(id, name)
        FractalCreated(id, verse_id, name)
        PetalCreated(id, fractal_id, name)
        NodeCreated(id, petal_id, name, has_asset)
        GltfImported(node_id, asset_id, petal_id, name, asset_path, position)
        HierarchyLoaded(verses)
        VerseInviteGenerated(verse_id, invite_string)
        VerseJoined(verse_id, verse_name)
        DatabaseReset(petal_name, rooms)
    }

    NetworkCommand ..> NetworkEvent : T2 processes → emits
    DbCommand ..> DbResult : T3 processes → emits
```

## Hierarchy Data Flow

The `LoadHierarchy` command returns the full tree as nested structs:

```
DbResult::HierarchyLoaded
  └── Vec<VerseHierarchyData>
        ├── id, name, namespace_id
        └── Vec<FractalHierarchyData>
              ├── id, name
              └── Vec<PetalHierarchyData>
                    ├── id, name
                    └── Vec<NodeHierarchyData>
                          ├── id, name, has_asset, position
                          ├── asset_path (optional)
                          └── petal_id
```
