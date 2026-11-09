//! The ingress plan: how each declared endpoint names its device and answers, pure and host-free.
//! Rationale: extensions/device-adapter/AGENTS.md §ingress-plan.

/// How an endpoint is wired in extension.json; mirrors its `transport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    HttpServer,
    WebsocketAccept,
    WebsocketDial,
    Mqtt,
}

/// Where the device id comes from before the body is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceNaming {
    /// The forwarded `x-device-id` header.
    Header,
    /// The `site-a/<id>/telemetry` topic.
    Topic,
    /// The body's `device_id` (one connection may carry many devices).
    Body,
}

/// What `on_ingress` does with a frame from one endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngressPlan {
    pub kind: EndpointKind,
    pub naming: DeviceNaming,
    /// A refused frame is answered to its sender (http status, websocket error body).
    pub answers_errors: bool,
    /// A good frame is acknowledged (http 202 only).
    pub acknowledges: bool,
}

/// Declared endpoints, id to kind; the test below keeps this table equal to extension.json.
pub const ENDPOINTS: [(&str, EndpointKind); 4] = [
    ("telemetry", EndpointKind::HttpServer),
    ("live", EndpointKind::WebsocketAccept),
    ("feed", EndpointKind::WebsocketDial),
    ("site-broker", EndpointKind::Mqtt),
];

/// The plan for a declared endpoint id; `None` for an id the manifest does not declare.
pub fn plan_for(endpoint: &str) -> Option<IngressPlan> {
    let kind = ENDPOINTS.iter().find(|(id, _)| *id == endpoint)?.1;
    Some(match kind {
        EndpointKind::HttpServer => IngressPlan { kind, naming: DeviceNaming::Header, answers_errors: true, acknowledges: true },
        // A caller on an accepted socket gets the reason; a dialled feed is a one-way hub stream, so errors are logged, not echoed.
        EndpointKind::WebsocketAccept => IngressPlan { kind, naming: DeviceNaming::Body, answers_errors: true, acknowledges: false },
        EndpointKind::WebsocketDial => IngressPlan { kind, naming: DeviceNaming::Body, answers_errors: false, acknowledges: false },
        EndpointKind::Mqtt => IngressPlan { kind, naming: DeviceNaming::Topic, answers_errors: false, acknowledges: false },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_table_matches_the_manifest_and_wires_each_transport() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../extension.json")).unwrap();
        let declared: Vec<(String, String)> = manifest["ingress"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry["id"].as_str().unwrap().to_string(), entry["transport"].as_str().unwrap().to_string()))
            .collect();
        let planned: Vec<(String, String)> = ENDPOINTS
            .iter()
            .map(|(id, kind)| {
                let transport = match kind {
                    EndpointKind::HttpServer => "http-server",
                    EndpointKind::WebsocketAccept => "websocket-accept",
                    EndpointKind::WebsocketDial => "websocket-dial",
                    EndpointKind::Mqtt => "mqtt",
                };
                (id.to_string(), transport.to_string())
            })
            .collect();
        assert_eq!(declared, planned);

        let feed = plan_for("feed").unwrap();
        assert_eq!((feed.naming, feed.answers_errors, feed.acknowledges), (DeviceNaming::Body, false, false));
        let live = plan_for("live").unwrap();
        assert_eq!((live.naming, live.answers_errors), (DeviceNaming::Body, true));
        let telemetry = plan_for("telemetry").unwrap();
        assert_eq!((telemetry.naming, telemetry.acknowledges), (DeviceNaming::Header, true));
        assert_eq!(plan_for("site-broker").unwrap().naming, DeviceNaming::Topic);
        assert_eq!(plan_for("ghost"), None);
    }
}
