use crate::projection::Projection;
use crate::ExportNode;

/// Convert scene nodes back to GPX XML.
///
/// Reconstructs the track hierarchy from node relationships:
/// - Nodes with gpx_type "track" become `<trk>` elements
/// - Their children with gpx_type "segment" become `<trkseg>` elements
/// - Their children with gpx_type "trackpoint" become `<trkpt>` elements
/// - Nodes with gpx_type "waypoint" become `<wpt>` elements
///
/// Extension data (hr, cad, power) is written as Garmin TrackPointExtension
/// elements when present in node properties.
///
/// Coordinates are preserved to 7 decimal places for round-trip fidelity.
pub fn scene_nodes_to_gpx(nodes: &[ExportNode], projection: &Projection) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<gpx version=\"1.1\" creator=\"FractalEngine\" ");
    xml.push_str("xmlns=\"http://www.topografix.com/GPX/1/1\" ");
    xml.push_str("xmlns:gpxtpx=\"http://www.garmin.com/xmlschemas/TrackPointExtension/v1\">\n");

    // Separate waypoints and tracks
    for node in nodes {
        let gpx_type = node
            .properties
            .as_ref()
            .and_then(|p| p["gpx_type"].as_str())
            .unwrap_or("");

        match gpx_type {
            "waypoint" => write_waypoint(&mut xml, node, projection),
            "track" => write_track(&mut xml, node, projection),
            _ => {}
        }
    }

    xml.push_str("</gpx>\n");
    xml
}

fn write_waypoint(xml: &mut String, node: &ExportNode, projection: &Projection) {
    let (lat, lon, ele) = extract_coords(node, projection);
    xml.push_str(&format!("  <wpt lat=\"{:.7}\" lon=\"{:.7}\">\n", lat, lon));
    if let Some(e) = ele {
        xml.push_str(&format!("    <ele>{:.1}</ele>\n", e));
    }
    if let Some(props) = &node.properties {
        if let Some(name) = props["name"].as_str() {
            xml.push_str(&format!("    <name>{}</name>\n", xml_escape(name)));
        }
        if let Some(desc) = props["desc"].as_str() {
            xml.push_str(&format!("    <desc>{}</desc>\n", xml_escape(desc)));
        }
        if let Some(sym) = props["symbol"].as_str() {
            xml.push_str(&format!("    <sym>{}</sym>\n", xml_escape(sym)));
        }
    }
    xml.push_str("  </wpt>\n");
}

fn write_track(xml: &mut String, node: &ExportNode, projection: &Projection) {
    xml.push_str("  <trk>\n");

    // Track name
    let name = &node.name;
    xml.push_str(&format!("    <name>{}</name>\n", xml_escape(name)));

    // Segments are children
    for child in &node.children {
        let child_type = child
            .properties
            .as_ref()
            .and_then(|p| p["gpx_type"].as_str())
            .unwrap_or("");

        if child_type == "segment" {
            write_segment(xml, child, projection);
        }
    }

    xml.push_str("  </trk>\n");
}

fn write_segment(xml: &mut String, node: &ExportNode, projection: &Projection) {
    xml.push_str("    <trkseg>\n");

    for child in &node.children {
        let child_type = child
            .properties
            .as_ref()
            .and_then(|p| p["gpx_type"].as_str())
            .unwrap_or("");

        if child_type == "trackpoint" {
            write_trackpoint(xml, child, projection);
        }
    }

    xml.push_str("    </trkseg>\n");
}

fn write_trackpoint(xml: &mut String, node: &ExportNode, projection: &Projection) {
    let (lat, lon, ele) = extract_coords(node, projection);
    xml.push_str(&format!(
        "      <trkpt lat=\"{:.7}\" lon=\"{:.7}\">\n",
        lat, lon
    ));
    if let Some(e) = ele {
        xml.push_str(&format!("        <ele>{:.1}</ele>\n", e));
    }
    if let Some(props) = &node.properties {
        if let Some(time) = props["time"].as_str() {
            xml.push_str(&format!("        <time>{}</time>\n", time));
        }

        // Garmin TrackPointExtension: hr, cad, power
        let hr = props["hr"].as_u64();
        let cad = props["cad"].as_u64();
        let power = props["power"].as_u64();
        if hr.is_some() || cad.is_some() || power.is_some() {
            xml.push_str("        <extensions>\n");
            xml.push_str("          <gpxtpx:TrackPointExtension>\n");
            if let Some(hr) = hr {
                xml.push_str(&format!("            <gpxtpx:hr>{}</gpxtpx:hr>\n", hr));
            }
            if let Some(cad) = cad {
                xml.push_str(&format!("            <gpxtpx:cad>{}</gpxtpx:cad>\n", cad));
            }
            if let Some(power) = power {
                xml.push_str(&format!(
                    "            <gpxtpx:power>{}</gpxtpx:power>\n",
                    power
                ));
            }
            xml.push_str("          </gpxtpx:TrackPointExtension>\n");
            xml.push_str("        </extensions>\n");
        }
    }
    xml.push_str("      </trkpt>\n");
}

/// Extract WGS84 coordinates from a node.
/// Prefers stored lat/lon properties for round-trip accuracy.
/// Falls back to inverse projection from local position.
fn extract_coords(node: &ExportNode, projection: &Projection) -> (f64, f64, Option<f64>) {
    if let Some(props) = &node.properties {
        let lat = props["lat"].as_f64();
        let lon = props["lon"].as_f64();
        let ele = props["ele"].as_f64();
        if let (Some(lat), Some(lon)) = (lat, lon) {
            return (lat, lon, ele);
        }
    }
    // Fallback: inverse projection
    let (lat, lon, ele) = projection.local_to_wgs84(
        node.position[0] as f64,
        node.position[1] as f64,
        node.position[2] as f64,
    );
    (lat, lon, Some(ele))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
