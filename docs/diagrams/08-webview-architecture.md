# WebView Architecture

## Browser Overlay System

```mermaid
graph TD
    subgraph T1["T1 — Bevy Main Thread"]
        Camera[Camera System]
        Plugin[WebViewPlugin]
        IPC[Typed IPC<br/>BrowserCommand / BrowserEvent]
    end

    subgraph Wry["wry WebView"]
        Overlay[Overlay Window<br/>Positioned per-frame]
        TrustBar["Trust Bar<br/>(non-dismissible)"]
        CSP[Content Security Policy<br/>Headers enforced]
        Denylist[URL Denylist<br/>localhost, 127.0.0.1, RFC 1918]
    end

    subgraph Model["3D Model Interaction"]
        Click[User clicks Model]
        Tabs[BrowserInteraction Tabs]
        Tab1["Tab 1: External URL<br/>(admin-configured)"]
        Tab2["Tab 2: Config Panel<br/>(role-gated settings)"]
    end

    Click --> Plugin
    Plugin --> Overlay
    Camera -->|world_to_viewport()| Overlay
    Overlay --> TrustBar
    Overlay --> Tabs
    Tabs --> Tab1 & Tab2

    IPC -->|BrowserCommand| Overlay
    Overlay -->|BrowserEvent| IPC
```

## IPC Command Flow

```mermaid
sequenceDiagram
    participant Bevy as Bevy System (T1)
    participant Plugin as WebViewPlugin
    participant Wry as wry WebView
    participant Web as Web Content

    Note over Bevy,Web: All JS↔Rust calls go through typed enum

    Bevy->>Plugin: BrowserCommand::Navigate { url }
    Plugin->>Plugin: Check URL against denylist
    alt URL blocked
        Plugin-->>Bevy: BrowserEvent::NavigationBlocked { url, reason }
    else URL allowed
        Plugin->>Wry: Load URL with CSP headers
        Wry->>Web: Render page
        Web-->>Wry: User interaction
        Wry-->>Plugin: BrowserEvent::PageLoaded { url, title }
        Plugin-->>Bevy: Forward event
    end
```

## Security Layers

```mermaid
graph LR
    subgraph Input["URL Input"]
        AdminURL[Admin-configured URL]
        UserNav[User navigation]
    end

    subgraph Checks["Security Checks"]
        D1[Block localhost]
        D2[Block 127.0.0.1]
        D3[Block 10.0.0.0/8]
        D4[Block 172.16.0.0/12]
        D5[Block 192.168.0.0/16]
        CSP2[Enforce CSP headers]
        Trust[Display trust bar]
        NoEval[No raw JS eval<br/>Typed IPC only]
    end

    subgraph Output["Rendered"]
        Safe[Safe WebView overlay]
    end

    Input --> Checks --> Output
```
