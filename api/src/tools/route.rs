use super::{MapPush, ToolOutcome, Tools};
use serde_json::{json, Value};

pub fn parse_waypoints(input: &Value, key: &str) -> Result<Vec<[f64; 2]>, String> {
    let arr = input
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("`{key}` must be an array of [lon, lat] pairs"))?;
    if arr.len() < 2 {
        return Err(format!("`{key}` needs at least two points"));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, p) in arr.iter().enumerate() {
        let c = p.as_array().ok_or_else(|| format!("{key}[{i}] is not an array"))?;
        let lon = c.first().and_then(Value::as_f64).ok_or_else(|| format!("{key}[{i}][0] not a number"))?;
        let lat = c.get(1).and_then(Value::as_f64).ok_or_else(|| format!("{key}[{i}][1] not a number"))?;
        if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
            return Err(format!("{key}[{i}] = [{lon}, {lat}] is out of range — order is [lon, lat]"));
        }
        out.push([lon, lat]);
    }
    Ok(out)
}

/// Recursively drop null-valued object entries.
///
/// Strict tool use requires every property to appear in `required`, so the model
/// sends the unused half of each either/or as `null` — `{"if": "...", "else_if":
/// null, "else": null, "multiply_by": 0.05, "limit_to": null}`. GraphHopper rejects
/// a rule that carries those keys at all, so they are stripped here rather than
/// being made optional in the schema (which strict mode does not allow).
pub fn prune_nulls(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, val)| !val.is_null())
                .map(|(k, val)| (k.clone(), prune_nulls(val)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(prune_nulls).collect()),
        other => other.clone(),
    }
}

/// GraphHopper refuses a query-time `distance_influence` below the value its
/// landmark preparation was built against:
///
///   "CustomModel in query can only use distance_influence bigger or equal to
///    90.0, but was: 40.0"
///
/// The bound exists because LM's A* heuristic is only admissible while query
/// weights stay at or above the prepared ones — a lower influence could make the
/// heuristic overestimate and return a wrong path. Nothing below the floor is
/// reachable at query time, so clamping and saying so beats failing the call and
/// making the agent spend a round trip rediscovering the limit.
pub const MIN_DISTANCE_INFLUENCE: f64 = 90.0;

/// Returns the usable model plus a note when the request had to be adjusted.
pub fn normalize_custom_model(cm: &Value) -> (Option<Value>, Option<String>) {
    if cm.is_null() {
        return (None, None);
    }
    let mut pruned = prune_nulls(cm);
    let mut note = None;

    if let Some(di) = pruned.get("distance_influence").and_then(Value::as_f64) {
        if di < MIN_DISTANCE_INFLUENCE {
            pruned["distance_influence"] = json!(MIN_DISTANCE_INFLUENCE);
            note = Some(format!(
                "distance_influence {di} was raised to {MIN_DISTANCE_INFLUENCE}: the graph's \
                 landmark preparation rejects anything lower. {MIN_DISTANCE_INFLUENCE} already \
                 allows the most detour available — express stronger preferences through the \
                 priority rules instead."
            ));
        }
    }

    // An all-null model prunes to `{}`, which GraphHopper rejects as empty.
    match pruned.as_object() {
        Some(o) if !o.is_empty() => (Some(pruned), note),
        _ => (None, note),
    }
}

/// Build the `/route` request body.
///
/// Three parameters here do NOT default to what we need, and each was verified
/// against the running service (design spec §8):
///   * `elevation: true`   — otherwise coordinates come back 2D even on a 3D graph,
///                           while `ascend`/`descend` still populate, so the omission
///                           is easy to miss.
///   * `ch.disable: true`  — Contraction Hierarchies cannot serve a runtime
///                           custom_model; the graph's LM preparation can.
///   * `points_encoded: false` — otherwise geometry is an encoded polyline, not GeoJSON.
pub fn body(waypoints: &[[f64; 2]], custom_model: Option<&Value>) -> Value {
    let mut b = json!({
        "points": waypoints,
        "profile": "car",
        "elevation": true,
        "points_encoded": false,
        "ch.disable": true,
        "instructions": false,
    });
    if let Some(cm) = custom_model {
        if let (Some(model), _) = normalize_custom_model(cm) {
            b["custom_model"] = model;
        }
    }
    b
}

pub async fn call_graphhopper(t: &Tools, body: &Value) -> Result<Value, String> {
    let url = format!("{}/route", t.cfg.graphhopper_url);
    let resp = t
        .http
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("graphhopper unreachable: {e}"))?;

    let status = resp.status();
    let parsed: Value = resp
        .json()
        .await
        .map_err(|e| format!("graphhopper returned invalid JSON: {e}"))?;

    if !status.is_success() {
        // GraphHopper reports "no route between these points" as a 400 with a
        // human-readable message. Pass it through verbatim — the agent uses it to
        // pick a different waypoint rather than giving up (spec §12).
        let msg = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown routing error");
        return Err(format!("routing failed: {msg}"));
    }
    Ok(parsed)
}

pub async fn run(t: &Tools, input: &Value) -> ToolOutcome {
    let waypoints = match parse_waypoints(input, "waypoints") {
        Ok(w) => w,
        Err(e) => return ToolOutcome::err(e),
    };

    let req = body(&waypoints, input.get("custom_model"));
    let parsed = match call_graphhopper(t, &req).await {
        Ok(v) => v,
        Err(e) => return ToolOutcome::err(e),
    };

    let path = match parsed.pointer("/paths/0") {
        Some(p) => p,
        None => return ToolOutcome::err("graphhopper returned no path"),
    };

    let distance = path.get("distance").and_then(Value::as_f64).unwrap_or(0.0);
    let time_ms = path.get("time").and_then(Value::as_f64).unwrap_or(0.0);
    let geometry = path.get("points").cloned().unwrap_or(json!(null));

    let geojson = json!({
        "type": "Feature",
        "geometry": geometry,
        "properties": { "kind": "route", "distance_m": distance, "duration_ms": time_ms }
    });

    // If we had to adjust the request, tell the agent — a silently different route
    // is worse than a slightly noisier tool result.
    let note = input
        .get("custom_model")
        .map(|cm| normalize_custom_model(cm).1)
        .unwrap_or(None);

    let summary = json!({
        "note": note,
        "distance_m": distance,
        "duration_ms": time_ms,
        "duration_human": human_duration(time_ms),
        "ascend_m": path.get("ascend").and_then(Value::as_f64),
        "descend_m": path.get("descend").and_then(Value::as_f64),
        // The full coordinate list can be tens of thousands of points. The agent
        // reasons about distance and duration, not vertices — the geometry goes to
        // the map instead of into the context window.
        "geometry_points": path
            .pointer("/points/coordinates")
            .and_then(Value::as_array)
            .map(|c| c.len())
            .unwrap_or(0),
    });

    ToolOutcome::ok_with(summary, MapPush::Geometry(geojson))
}

pub fn human_duration(ms: f64) -> String {
    let total_min = (ms / 60000.0).round() as i64;
    let h = total_min / 60;
    let m = total_min % 60;
    if h > 0 {
        format!("{h}h{m:02}m")
    } else {
        format!("{m}m")
    }
}
