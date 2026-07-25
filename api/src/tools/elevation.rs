use super::{route, ToolOutcome, Tools};
use serde_json::{json, Value};

/// Climb profile for a leg.
///
/// GraphHopper has no dedicated elevation endpoint — the third ordinate of a routed
/// line *is* the elevation, which is why `elevation: true` on the route request is
/// load-bearing here rather than a nicety.
pub async fn run(t: &Tools, input: &Value) -> ToolOutcome {
    let waypoints = match route::parse_waypoints(input, "waypoints") {
        Ok(w) => w,
        Err(e) => return ToolOutcome::err(e),
    };
    let samples = input.get("samples").and_then(Value::as_i64).unwrap_or(20).clamp(2, 100) as usize;

    let req = route::body(&waypoints, None);
    let parsed = match route::call_graphhopper(t, &req).await {
        Ok(v) => v,
        Err(e) => return ToolOutcome::err(e),
    };

    let coords = match parsed.pointer("/paths/0/points/coordinates").and_then(Value::as_array) {
        Some(c) if !c.is_empty() => c,
        _ => return ToolOutcome::err("graphhopper returned no geometry"),
    };

    let elevations: Vec<f64> = coords
        .iter()
        .filter_map(|c| c.as_array().and_then(|a| a.get(2)).and_then(Value::as_f64))
        .collect();

    if elevations.is_empty() {
        return ToolOutcome::err(
            "route geometry carries no elevation — the graph was built without a DEM",
        );
    }

    // Evenly spaced samples across the line, endpoints always included.
    let n = elevations.len();
    let picked: Vec<f64> = (0..samples)
        .map(|i| {
            let idx = if samples == 1 { 0 } else { i * (n - 1) / (samples - 1) };
            (elevations[idx] * 10.0).round() / 10.0
        })
        .collect();

    let path = parsed.pointer("/paths/0").cloned().unwrap_or(json!({}));
    let max = elevations.iter().cloned().fold(f64::MIN, f64::max);
    let min = elevations.iter().cloned().fold(f64::MAX, f64::min);

    ToolOutcome::ok(json!({
        "ascend_m": path.get("ascend").and_then(Value::as_f64),
        "descend_m": path.get("descend").and_then(Value::as_f64),
        "min_elevation_m": (min * 10.0).round() / 10.0,
        "max_elevation_m": (max * 10.0).round() / 10.0,
        "distance_m": path.get("distance").and_then(Value::as_f64),
        "samples": picked,
    }))
}
