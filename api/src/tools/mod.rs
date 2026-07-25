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
    pub map_push: Option<MapPush>
}

pub enum MapPush {
    Geometry(Value),
    Markers(Value)
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
            map_push: None
        }
    }
}

#[derive(Clone)]
pub struct Tools {
    pub cfg: Config,
    pub http: reqwest::Client,
    pub pool: Option<Pool>
}

impl Tools {
    pub async fn dispatch(&self, name: &str, input: &Value) -> ToolOutcome {
        match name {
            "geocode" => geocode::run(self, input).await,
            "search_poi" => search_poi::run(self, input).await,
            "route" => route::run(self, input).await,
            "elevation_profile" => elevation::run(self, input).await,
            other => ToolOutcome::err(format!("unknown tool '{other}'"))
        }
    }
}

/// GraphHopper's CustomModel, declared structurally.
///
/// Strict tool use forbids `additionalProperties: true`, so a free-form object is
/// not an option — every shape has to be spelled out. That is a feature rather than
/// a workaround: the model can now only emit rule objects that are valid by
/// construction, instead of inventing CustomModel keys GraphHopper would reject.
///
/// Strict also wants every property in `required`, so the alternatives within a
/// rule (`if` / `else_if` / `else`, `multiply_by` / `limit_to`) are all nullable and
/// all required. `prune_nulls` strips the unused ones before the request leaves us —
/// GraphHopper rejects a rule carrying `"if": null`.
fn rule_schema(what: &str) -> Value {
    json!({
        "type": ["array", "null"],
        "description": format!("{what} Each entry selects edges with exactly one of 'if', 'else_if' or 'else', then applies exactly one of 'multiply_by' or 'limit_to'. Set the others to null."),
        "items": {
            "type": "object",
            "properties": {
                "if": {
                    "type": ["string", "null"],
                    "description": "Condition over encoded values, e.g. \"road_class == MOTORWAY\", \"toll != NO\", \"curvature < 0.8\", \"average_slope > 6\". Use \"true\" to match everything."
                },
                "else_if": { "type": ["string", "null"], "description": "Condition applied only if the preceding rule did not match." },
                "else": { "type": ["string", "null"], "description": "Set to \"\" to apply to everything the preceding rules missed." },
                "multiply_by": {
                    "type": ["number", "null"],
                    "description": "Scale factor, 0 to 1. 0.05 strongly discourages, 1 is neutral."
                },
                "limit_to": {
                    "type": ["number", "null"],
                    "description": "Hard ceiling for the matched edges. In a speed rule this is km/h."
                }
            },
            "required": ["if", "else_if", "else", "multiply_by", "limit_to"],
            "additionalProperties": false
        }
    })
}

fn custom_model_schema() -> Value {
    json!({
        "type": ["object", "null"],
        "description": "GraphHopper CustomModel expressing the trip's preferences. Null for a plain fastest route. Encoded values available: road_class, road_environment, surface, smoothness, curvature, average_slope, max_slope, toll, track_type, max_speed. Example — scenic backroads: priority [{if: \"road_class == MOTORWAY\", multiply_by: 0.05}, {if: \"curvature < 0.8\", multiply_by: 0.4}] with distance_influence 30.",
        "properties": {
            "priority": rule_schema("How strongly to prefer or avoid matching roads."),
            "speed": rule_schema("Caps on travel speed for matching roads."),
            "distance_influence": {
                "type": ["number", "null"],
                "description": "How much detour the preferences may cost. 0 ignores distance entirely; 30 is a moderate trade; 100+ keeps routes close to the shortest path."
            }
        },
        "required": ["priority", "speed", "distance_influence"],
        "additionalProperties": false
    })
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
                        "items": { "type": "number" }
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
                        "items": { "type": "number" }
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
                            "items": { "type": "number" }
                        }
                    },
                    "profile": {
                        "type": "string",
                        "description": "Routing profile. Only 'car' is available.",
                        "enum": ["car"]
                    },
                    "custom_model": custom_model_schema()
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
                            "items": { "type": "number" }
                        }
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
