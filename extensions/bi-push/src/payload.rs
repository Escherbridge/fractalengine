//! Pure rendering of a `host/query` snapshot into a push body; no host calls.
//! Design notes: extensions/bi-push/AGENTS.md §payload.

use serde_json::{json, Map, Value};

/// Body format of a push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Json,
    Csv,
}

impl Format {
    /// `json` or `csv`; anything else is `None`.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "json" => Some(Self::Json),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }

    /// The request `content-type`.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Csv => "text/csv",
        }
    }
}

/// The rows of one `host/query` snapshot (`{"data": [...], "row_count", "truncated"}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub rows: Vec<Value>,
    pub truncated: bool,
}

/// Parse a snapshot event's data; a payload without a `data` array is refused.
pub fn parse_snapshot(data: &str) -> Result<Snapshot, String> {
    let value: Value = serde_json::from_str(data).map_err(|error| format!("snapshot is not JSON: {error}"))?;
    let rows = value.get("data").and_then(Value::as_array).cloned().ok_or("snapshot has no data array")?;
    let truncated = value.get("truncated").and_then(Value::as_bool).unwrap_or(false);
    Ok(Snapshot { rows, truncated })
}

/// Render the snapshot; JSON keeps the `/api/v1/query` field names so the sink sees the REST shape.
pub fn render(snapshot: &Snapshot, format: Format) -> Vec<u8> {
    match format {
        Format::Json => json!({ "data": snapshot.rows, "row_count": snapshot.rows.len(), "truncated": snapshot.truncated })
            .to_string()
            .into_bytes(),
        Format::Csv => render_csv(&snapshot.rows).into_bytes(),
    }
}

/// RFC 4180 CSV: header is the union of row keys in first-seen order (keys sorted within a row: serde_json's
/// default map); null is empty, nested values are compact JSON, text cells are formula-neutralised.
fn render_csv(rows: &[Value]) -> String {
    let mut columns: Vec<&str> = Vec::new();
    for row in rows.iter().filter_map(Value::as_object) {
        for key in row.keys() {
            if !columns.contains(&key.as_str()) {
                columns.push(key);
            }
        }
    }
    let mut out = columns.iter().map(|column| text_field(column)).collect::<Vec<_>>().join(",");
    out.push_str("\r\n");
    let empty = Map::new();
    for row in rows {
        let object = row.as_object().unwrap_or(&empty);
        let line = columns.iter().map(|column| cell_field(object.get(*column))).collect::<Vec<_>>().join(",");
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out
}

/// One cell: strings go through [`text_field`]; numbers (negatives included), booleans and nested JSON stay as is.
fn cell_field(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text_field(text),
        Some(other) => csv_field(&other.to_string()),
    }
}

/// OWASP CSV injection: text starting with `= + - @ TAB CR` gets a leading `'` and is always quoted.
fn text_field(text: &str) -> String {
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        quoted(&format!("'{text}"))
    } else {
        csv_field(text)
    }
}

/// Quote a field when it holds a comma, quote or line break.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\r', '\n']) {
        quoted(text)
    } else {
        text.to_string()
    }
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `host/query` snapshot reaches the sink as the REST JSON shape, or as CSV that quotes, unions columns,
    /// blanks nulls and never hands a spreadsheet an editor-written formula; a non-query payload is refused.
    #[test]
    fn snapshot_renders_rest_json_and_injection_safe_csv() {
        let raw = r#"{"data":[
            {"node_id":"a","metric":"t,c","value":1.5},
            {"node_id":"b","units":"C\"x","value":null},
            {"node_id":"=HYPERLINK(\"http://x\")","metric":"@SUM(A1)","units":"-2","value":-3.5},
            {"node_id":"\tc","metric":"+1","units":"\rd","value":2}
        ],"row_count":4,"truncated":false}"#;
        let snapshot = parse_snapshot(raw).unwrap();

        let body: Value = serde_json::from_slice(&render(&snapshot, Format::Json)).unwrap();
        assert_eq!(body, serde_json::from_str::<Value>(raw).unwrap(), "JSON keeps the /api/v1/query shape and rows");

        let csv = String::from_utf8(render(&snapshot, Format::Csv)).unwrap();
        assert_eq!(
            csv,
            "metric,node_id,value,units\r\n\
             \"t,c\",a,1.5,\r\n\
             ,b,,\"C\"\"x\"\r\n\
             \"'@SUM(A1)\",\"'=HYPERLINK(\"\"http://x\"\")\",-3.5,\"'-2\"\r\n\
             \"'+1\",\"'\tc\",2,\"'\rd\"\r\n"
        );

        assert!(parse_snapshot(r#"{"nodes":[]}"#).is_err(), "a payload that is not a query result is never pushed");
    }
}
