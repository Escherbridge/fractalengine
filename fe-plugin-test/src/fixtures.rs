//! Scene fixtures — pre-built scene configurations for plugin tests.
//!
//! Fixtures provide reproducible starting states so plugin authors don't have
//! to manually construct dozens of nodes for every test case.

use std::collections::HashMap;

use fe_sdk::node::NodeSnapshot;
use fe_sdk::property::PropertyValue;

use crate::mock_host::MockHostEnv;

/// A pre-built scene configuration that can be loaded into a [`MockHostEnv`].
#[derive(Debug, Clone)]
pub struct SceneFixture {
    /// The nodes in this fixture.
    pub nodes: Vec<NodeSnapshot>,
    /// Extra properties keyed by `(node_id, key)`.
    pub properties: HashMap<(String, String), String>,
}

impl SceneFixture {
    /// Deserialize a fixture from a JSON string.
    ///
    /// Expected format:
    /// ```json
    /// {
    ///   "nodes": [ { "node_id": "...", "petal_id": "...", ... } ],
    ///   "properties": { "node_id:key": "value", ... }
    /// }
    /// ```
    pub fn load_from_json(json: &str) -> Self {
        let raw: serde_json::Value = serde_json::from_str(json).expect("Invalid fixture JSON");

        let nodes: Vec<NodeSnapshot> = if let Some(arr) = raw.get("nodes") {
            serde_json::from_value(arr.clone()).expect("Invalid nodes array in fixture JSON")
        } else {
            Vec::new()
        };

        let mut properties = HashMap::new();
        if let Some(props_obj) = raw.get("properties").and_then(|v| v.as_object()) {
            for (compound_key, value) in props_obj {
                if let Some((node_id, key)) = compound_key.split_once(':') {
                    let val_str = match value {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    properties.insert((node_id.to_string(), key.to_string()), val_str);
                }
            }
        }

        Self { nodes, properties }
    }

    /// Convert this fixture into a [`MockHostEnv`] ready for testing.
    pub fn to_mock_host(&self) -> MockHostEnv {
        let mut host = MockHostEnv::new();
        for node in &self.nodes {
            host.insert_node(node.node_id.clone(), node.clone());
        }
        for ((node_id, key), value) in &self.properties {
            host.insert_property(node_id.clone(), key.clone(), value.clone());
        }
        host
    }
}

// ---------------------------------------------------------------------------
// Built-in fixtures
// ---------------------------------------------------------------------------

/// An empty scene with zero nodes.
pub fn empty_scene() -> SceneFixture {
    SceneFixture {
        nodes: Vec::new(),
        properties: HashMap::new(),
    }
}

/// A basic terrain scene with 10 nodes containing elevation, latitude, and
/// longitude properties.
pub fn basic_terrain() -> SceneFixture {
    let mut nodes = Vec::with_capacity(10);
    let mut properties = HashMap::new();

    for i in 0..10 {
        let node_id = format!("terrain_{:03}", i);
        let mut props = HashMap::new();
        let elevation = 100.0 + (i as f64) * 50.0;
        let lat = 47.0 + (i as f64) * 0.01;
        let lon = -122.0 + (i as f64) * 0.01;

        props.insert("elevation".to_string(), PropertyValue::Number(elevation));
        props.insert("lat".to_string(), PropertyValue::Number(lat));
        props.insert("lon".to_string(), PropertyValue::Number(lon));
        props.insert(
            "type".to_string(),
            PropertyValue::String("terrain_point".into()),
        );

        let snapshot = NodeSnapshot {
            node_id: node_id.clone(),
            petal_id: "terrain_petal".into(),
            name: format!("Terrain Point {}", i),
            position: [lon, elevation, lat],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            properties: props,
        };

        // Also store as string properties for the property map
        properties.insert(
            (node_id.clone(), "elevation".to_string()),
            elevation.to_string(),
        );
        properties.insert((node_id.clone(), "lat".to_string()), lat.to_string());
        properties.insert((node_id.clone(), "lon".to_string()), lon.to_string());

        nodes.push(snapshot);
    }

    SceneFixture { nodes, properties }
}

/// A sensor network scene with 50 sensor nodes containing temperature,
/// humidity, and status properties.
pub fn sensor_network() -> SceneFixture {
    let mut nodes = Vec::with_capacity(50);
    let mut properties = HashMap::new();

    for i in 0..50 {
        let node_id = format!("sensor_{:03}", i);
        let mut props = HashMap::new();

        let temperature = 15.0 + (i as f64) * 0.5;
        let humidity = 30.0 + (i as f64) * 1.2;
        let status = if i % 5 == 0 { "offline" } else { "online" };

        props.insert(
            "temperature".to_string(),
            PropertyValue::Number(temperature),
        );
        props.insert("humidity".to_string(), PropertyValue::Number(humidity));
        props.insert("status".to_string(), PropertyValue::String(status.into()));
        props.insert("type".to_string(), PropertyValue::String("sensor".into()));
        props.insert(
            "sensor_id".to_string(),
            PropertyValue::String(format!("SN-{:04}", i)),
        );

        let x = (i % 10) as f64 * 10.0;
        let z = (i / 10) as f64 * 10.0;

        let snapshot = NodeSnapshot {
            node_id: node_id.clone(),
            petal_id: "sensor_petal".into(),
            name: format!("Sensor {}", i),
            position: [x, 0.0, z],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            properties: props,
        };

        properties.insert(
            (node_id.clone(), "temperature".to_string()),
            temperature.to_string(),
        );
        properties.insert(
            (node_id.clone(), "humidity".to_string()),
            humidity.to_string(),
        );
        properties.insert((node_id.clone(), "status".to_string()), status.to_string());

        nodes.push(snapshot);
    }

    SceneFixture { nodes, properties }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scene_has_no_nodes() {
        let fixture = empty_scene();
        assert!(fixture.nodes.is_empty());
        assert!(fixture.properties.is_empty());

        let host = fixture.to_mock_host();
        assert!(host.nodes.is_empty());
    }

    #[test]
    fn basic_terrain_has_10_nodes() {
        let fixture = basic_terrain();
        assert_eq!(fixture.nodes.len(), 10);
        assert!(!fixture.properties.is_empty());

        let host = fixture.to_mock_host();
        assert_eq!(host.nodes.len(), 10);

        let node = host.get_node("terrain_000").unwrap();
        assert_eq!(node.petal_id, "terrain_petal");
        assert!(node.properties.contains_key("elevation"));
        assert!(node.properties.contains_key("lat"));
        assert!(node.properties.contains_key("lon"));
    }

    #[test]
    fn sensor_network_has_50_nodes() {
        let fixture = sensor_network();
        assert_eq!(fixture.nodes.len(), 50);

        let host = fixture.to_mock_host();
        assert_eq!(host.nodes.len(), 50);

        let node = host.get_node("sensor_000").unwrap();
        assert_eq!(node.petal_id, "sensor_petal");
        assert!(node.properties.contains_key("temperature"));
        assert!(node.properties.contains_key("humidity"));
        assert!(node.properties.contains_key("status"));

        // sensor_000 (i=0) should be offline (0 % 5 == 0)
        if let Some(PropertyValue::String(s)) = node.properties.get("status") {
            assert_eq!(s, "offline");
        } else {
            panic!("Expected status property to be a string");
        }
    }

    #[test]
    fn load_from_json_works() {
        let json = r#"{
            "nodes": [
                {
                    "node_id": "n1",
                    "petal_id": "p1",
                    "name": "Node 1",
                    "position": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                    "properties": {}
                }
            ],
            "properties": {
                "n1:color": "red",
                "n1:size": "42"
            }
        }"#;

        let fixture = SceneFixture::load_from_json(json);
        assert_eq!(fixture.nodes.len(), 1);
        assert_eq!(fixture.nodes[0].node_id, "n1");
        assert_eq!(
            fixture.properties.get(&("n1".into(), "color".into())),
            Some(&"red".to_string())
        );
        assert_eq!(
            fixture.properties.get(&("n1".into(), "size".into())),
            Some(&"42".to_string())
        );

        let host = fixture.to_mock_host();
        assert!(host.get_node("n1").is_some());
    }
}
