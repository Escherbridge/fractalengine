use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Read;

/// Parsed GPX data with all tracks, routes, and waypoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpxData {
    pub tracks: Vec<GpxTrack>,
    pub routes: Vec<GpxRoute>,
    pub waypoints: Vec<GpxWaypoint>,
    pub metadata: Option<GpxMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpxMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpxTrack {
    pub name: Option<String>,
    pub description: Option<String>,
    pub segments: Vec<GpxSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpxSegment {
    pub points: Vec<GpxTrackpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpxTrackpoint {
    pub lat: f64,
    pub lon: f64,
    pub ele: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hr: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cad: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpxRoute {
    pub name: Option<String>,
    pub points: Vec<GpxWaypoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpxWaypoint {
    pub lat: f64,
    pub lon: f64,
    pub ele: Option<f64>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<DateTime<Utc>>,
}

/// Parse GPX 1.0 or 1.1 from a reader.
pub fn parse_gpx<R: Read>(reader: R) -> Result<GpxData> {
    let gpx_data = gpx::read(reader).context("failed to parse GPX")?;

    let metadata = gpx_data.metadata.map(|m| GpxMetadata {
        name: m.name,
        description: m.description,
        author: m.author.map(|a| a.name.unwrap_or_default()),
    });

    let tracks = gpx_data
        .tracks
        .into_iter()
        .map(|t| GpxTrack {
            name: t.name,
            description: t.description,
            segments: t
                .segments
                .into_iter()
                .map(|s| GpxSegment {
                    points: s
                        .points
                        .into_iter()
                        .map(|wp| {
                            let pt = wp.point();
                            GpxTrackpoint {
                                lat: pt.y(),
                                lon: pt.x(),
                                ele: wp.elevation,
                                time: wp.time.and_then(|t| {
                                    t.format()
                                        .ok()
                                        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                                }),
                                hr: None,
                                cad: None,
                                power: None,
                                speed: wp.speed,
                            }
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();

    let routes = gpx_data
        .routes
        .into_iter()
        .map(|r| GpxRoute {
            name: r.name,
            points: r
                .points
                .into_iter()
                .map(|wp| {
                    let pt = wp.point();
                    GpxWaypoint {
                        lat: pt.y(),
                        lon: pt.x(),
                        ele: wp.elevation,
                        name: wp.name,
                        description: wp.description,
                        symbol: wp.symbol,
                        time: wp.time.and_then(|t| {
                            t.format()
                                .ok()
                                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                        }),
                    }
                })
                .collect(),
        })
        .collect();

    let waypoints = gpx_data
        .waypoints
        .into_iter()
        .map(|wp| {
            let pt = wp.point();
            GpxWaypoint {
                lat: pt.y(),
                lon: pt.x(),
                ele: wp.elevation,
                name: wp.name,
                description: wp.description,
                symbol: wp.symbol,
                time: wp.time.and_then(|t| {
                    t.format()
                        .ok()
                        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                }),
            }
        })
        .collect();

    Ok(GpxData {
        tracks,
        routes,
        waypoints,
        metadata,
    })
}

/// Parse GPX from bytes.
pub fn parse_gpx_bytes(bytes: &[u8]) -> Result<GpxData> {
    parse_gpx(std::io::Cursor::new(bytes))
}
