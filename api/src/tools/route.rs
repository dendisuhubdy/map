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
        if !cm.is_null() {
            b["custom_model"] = cm.clone();
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

    let summary = json!({
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
