pub mod elevation;
pub mod geocode;
pub mod route;
pub mod search_poi;

use crate::config::Config;
use deadpool_postgres::Pool;
use serde_json::{json, Value};

/// What a tool hands back to the agent loop.
///
/// `map_push` is the incremental-rendering hook from design spec §10: a tool that
/// produces geometry emits it here so the loop can stream it to the browser the
/// moment it exists, rather than after the agent finishes reasoning about it.
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
    pub map_push: Option<MapPush>,
}

pub enum MapPush {
    Geometry(Value),
    Markers(Value),
}

impl ToolOutcome {
    pub fn ok(content: Value) -> Self {
        Self { content: content.to_string(), is_error: false, map_push: None }
    }

    pub fn ok_with(content: Value, push: MapPush) -> Self {
        Self { content: content.to_string(), is_error: false, map_push: Some(push) }
    }

    /// Invariant 3 (design spec §10): a failed tool is a `tool_result` with
    /// `is_error: true`, never a dropped block and never a hard failure. The agent
    /// reads the message and adapts — a 500 from us would end the conversation.
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            content: json!({ "error": message.into() }).to_string(),
            is_error: true,
            map_push: None,
        }
    }
}

#[derive(Clone)]
pub struct Tools {
    pub cfg: Config,
    pub http: reqwest::Client,
    pub pool: Option<Pool>,
}

impl Tools {
    pub async fn dispatch(&self, name: &str, input: &Value) -> ToolOutcome {
        match name {
            "geocode" => geocode::run(self, input).await,
            "search_poi" => search_poi::run(self, input).await,
            "route" => route::run(self, input).await,
            "elevation_profile" => elevation::run(self, input).await,
            other => ToolOutcome::err(format!("unknown tool '{other}'")),
        }
    }
}

/// The four tools, declared `strict: true` with `additionalProperties: false` and
/// every property listed in `required` — optional arguments are expressed as
/// nullable types rather than omitted keys. That combination is what lets the Rust
/// side deserialize into concrete shapes without defensive parsing (spec §8).
pub fn definitions() -> Value {
    json!([
        {
            "name": "geocode",
            "description": "Resolve a place name, address, or landmark to coordinates. Use this to turn any name the user mentions into a point or bounding box before routing or searching. Returns ranked candidates — if several are plausible, ask the user which they meant rather than guessing.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Place name to resolve, e.g. 'Mount Bromo' or 'Malang'."
                    },
                    "bbox": {
                        "type": ["array", "null"],
                        "description": "Optional bias box [min_lon, min_lat, max_lon, max_lat]. Restricts results to a region.",
                        "items": { "type": "number" },
                        "minItems": 4,
                        "maxItems": 4
                    }
                },
                "required": ["query", "bbox"],
                "additionalProperties": false
            }
        },
        {
            "name": "search_poi",
            "description": "Find points of interest by OpenStreetMap tag inside a bounding box. Tags are 'key=value' strings from OSM's vocabulary, e.g. 'natural=volcano', 'natural=beach', 'tourism=viewpoint', 'historic=ruins', 'amenity=restaurant', 'leisure=park'. Multiple tags are OR-ed. Zero results is a normal answer, not an error — widen the box or relax the tags.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "tags": {
                        "type": "array",
                        "description": "OSM tags to match, each 'key=value'.",
                        "items": { "type": "string" }
                    },
                    "bbox": {
                        "type": "array",
                        "description": "Search box [min_lon, min_lat, max_lon, max_lat].",
                        "items": { "type": "number" },
                        "minItems": 4,
                        "maxItems": 4
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum results to return (1-200)."
                    }
                },
                "required": ["tags", "bbox", "limit"],
                "additionalProperties": false
            }
        },
        {
            "name": "route",
            "description": "Compute a driving route through an ordered list of waypoints. This is where trip preferences are expressed: pass a GraphHopper custom_model to bias the route (avoid motorways, prefer twisty or scenic roads, avoid tolls). Returns GeoJSON geometry, distance in metres, duration in milliseconds, and ascent/descent in metres.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "waypoints": {
                        "type": "array",
                        "description": "Ordered [lon, lat] pairs. At least two.",
                        "items": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "minItems": 2
                    },
                    "profile": {
                        "type": "string",
                        "description": "Routing profile. Only 'car' is available.",
                        "enum": ["car"]
                    },
                    "custom_model": {
                        "type": ["object", "null"],
                        "description": "GraphHopper CustomModel JSON. Use 'priority' rules over encoded values road_class, road_environment, surface, curvature, average_slope, max_slope, toll, track_type, and 'distance_influence' to trade directness against preference. Example: {\"priority\":[{\"if\":\"road_class == MOTORWAY\",\"multiply_by\":0.05},{\"if\":\"toll != NO\",\"multiply_by\":0.1}],\"distance_influence\":30}",
                        "additionalProperties": true
                    }
                },
                "required": ["waypoints", "profile", "custom_model"],
                "additionalProperties": false
            }
        },
        {
            "name": "elevation_profile",
            "description": "Return the climb profile for a route: total ascent and descent in metres plus evenly spaced elevation samples along the line. Use it to check whether a leg is mountainous before committing to it.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "waypoints": {
                        "type": "array",
                        "description": "Ordered [lon, lat] pairs describing the leg.",
                        "items": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "minItems": 2
                    },
                    "samples": {
                        "type": "integer",
                        "description": "How many evenly spaced elevation samples to return (2-100)."
                    }
                },
                "required": ["waypoints", "samples"],
                "additionalProperties": false
            }
        }
    ])
}
