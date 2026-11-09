//! The declarative frame mapping: one JSON shape for every transport, parsed without host calls.
//! Shape and rationale: extensions/device-adapter/AGENTS.md §frame.

use serde_json::Value;

/// Readings accepted in one frame.
pub const MAX_READINGS_PER_FRAME: usize = 200;

/// One reading from a frame, before its anchor node is known.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedReading {
    pub metric: String,
    pub value: f64,
    pub units: String,
    pub at_ms: Option<i64>,
}

/// A frame mapped to what it asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub device_id: String,
    pub readings: Vec<ParsedReading>,
    pub position: Option<[f64; 3]>,
    pub heading_deg: Option<f64>,
}

/// Device ids are topic-safe: they end up inside MQTT topics and node property values.
pub fn valid_device_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The device id in `site-a/<id>/telemetry`, if the topic has that shape.
pub fn device_id_from_topic(topic: &str) -> Option<&str> {
    let mut parts = topic.split('/');
    let (_site, id, _leaf) = (parts.next()?, parts.next()?, parts.next()?);
    parts.next().is_none().then_some(id)
}

/// Rotation about +Y for a compass heading in degrees, `[x, y, z, w]`.
pub fn heading_quaternion(heading_deg: f64) -> [f64; 4] {
    let half = heading_deg.to_radians() / 2.0;
    [0.0, half.sin(), 0.0, half.cos()]
}

fn finite(value: &Value, what: &str) -> Result<f64, String> {
    value.as_f64().filter(|number| number.is_finite()).ok_or_else(|| format!("{what} must be a finite number"))
}

/// Map a frame body to a [`Parsed`]; `hint` is the id the transport already names (header or topic) and wins over the body.
pub fn parse_frame(body: &[u8], hint: Option<&str>) -> Result<Parsed, String> {
    let text = std::str::from_utf8(body).map_err(|_| "frame is not UTF-8".to_string())?;
    let value: Value = serde_json::from_str(text).map_err(|error| format!("frame is not JSON: {error}"))?;
    let object = value.as_object().ok_or("frame must be a JSON object")?;

    let named_in_body = object.get("device_id").map(|id| id.as_str().ok_or("device_id must be a string")).transpose()?;
    let device_id = match (hint, named_in_body) {
        (Some(from_transport), Some(from_body)) if from_transport != from_body => {
            return Err(format!("device_id {from_body} does not match the endpoint's {from_transport}"));
        }
        (Some(id), _) | (None, Some(id)) => id,
        (None, None) => return Err("frame names no device".into()),
    };
    if !valid_device_id(device_id) {
        return Err("device_id must be 1-64 characters of letters, digits, '.', '_' or '-'".into());
    }

    let mut readings = Vec::new();
    if let Some(rows) = object.get("readings") {
        let rows = rows.as_array().ok_or("readings must be an array")?;
        if rows.len() > MAX_READINGS_PER_FRAME {
            return Err(format!("a frame carries at most {MAX_READINGS_PER_FRAME} readings"));
        }
        for row in rows {
            let metric = row.get("metric").and_then(Value::as_str).filter(|metric| (1..=64).contains(&metric.len())).ok_or("reading metric must be 1-64 characters")?;
            let units = row.get("units").and_then(Value::as_str).filter(|units| (1..=16).contains(&units.len())).ok_or("reading units must be 1-16 characters")?;
            let value = finite(row.get("value").unwrap_or(&Value::Null), "reading value")?;
            let at_ms = row.get("at_ms").map(|at| at.as_i64().ok_or("at_ms must be an integer")).transpose()?;
            readings.push(ParsedReading { metric: metric.into(), value, units: units.into(), at_ms });
        }
    }

    let position = match object.get("position") {
        None => None,
        Some(point) => {
            let parts = point.as_array().filter(|parts| parts.len() == 3).ok_or("position must be [x, y, z]")?;
            Some([finite(&parts[0], "position x")?, finite(&parts[1], "position y")?, finite(&parts[2], "position z")?])
        }
    };
    let heading_deg = object.get("heading_deg").map(|heading| finite(heading, "heading_deg")).transpose()?;

    if readings.is_empty() && position.is_none() && heading_deg.is_none() {
        return Err("frame carries nothing to apply".into());
    }
    Ok(Parsed { device_id: device_id.into(), readings, position, heading_deg })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_map_or_are_refused_with_a_reason() {
        let ok = parse_frame(br#"{"readings":[{"metric":"temperature_c","value":21.5,"units":"C"}],"position":[1,2,3]}"#, Some("dev-1")).unwrap();
        assert_eq!((ok.device_id.as_str(), ok.readings.len(), ok.position), ("dev-1", 1, Some([1.0, 2.0, 3.0])));
        for (body, hint, expected) in [
            (&br#"{"readings":[]}"#[..], Some("dev-1"), "nothing to apply"),
            (br#"{"device_id":"dev-2","position":[0,0,0]}"#, Some("dev-1"), "does not match"),
            (br#"{"position":[0,0,0]}"#, None, "names no device"),
            (br#"{"device_id":"a/b","position":[0,0,0]}"#, None, "device_id must be"),
            (br#"{"readings":[{"metric":"m","value":null,"units":"u"}]}"#, Some("d"), "finite number"),
            (b"not json", Some("d"), "not JSON"),
            (br#"{"device_id":"dev-9","readings":[{"metric":"m","value":1,"units":"u"}],"position":[0,0]}"#, None, "position must be"),
        ] {
            let error = parse_frame(body, hint).unwrap_err();
            assert!(error.contains(expected), "{error} should contain {expected}");
        }
        // A dialled feed names the device only in the body.
        let dialled = parse_frame(br#"{"device_id":"dev-9","heading_deg":90}"#, None).unwrap();
        assert_eq!((dialled.device_id.as_str(), dialled.heading_deg), ("dev-9", Some(90.0)));
        assert_eq!(device_id_from_topic("site-a/dev-1/telemetry"), Some("dev-1"));
        assert_eq!(device_id_from_topic("site-a/dev-1"), None);
        assert_eq!(device_id_from_topic("a/b/c/d"), None);
    }
}
