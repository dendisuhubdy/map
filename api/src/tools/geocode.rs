use super::{MapPush, ToolOutcome, Tools};
use serde_json::{json, Value};

/// Photon geocoding. Returns ranked candidates rather than a single best guess so
/// the agent can disambiguate — or ask the user — instead of silently picking one
/// (design spec §12).
pub async fn run(t: &Tools, input: &Value) -> ToolOutcome {
    let query = match input.get("query").and_then(Value::as_str) {
        Some(q) if !q.trim().is_empty() => q,
        _ => return ToolOutcome::err("`query` must be a non-empty string"),
    };

    let mut url = format!(
        "{}/api?q={}&limit=8",
        t.cfg.photon_url,
        urlencoding::encode(query)
    );

    // Photon's bias box is `bbox=minLon,minLat,maxLon,maxLat` — same ordering the
    // tool schema uses, so no reordering here.
    if let Some(b) = input.get("bbox").and_then(Value::as_array) {
        if b.len() == 4 {
            let n: Vec<String> = b
                .iter()
                .filter_map(Value::as_f64)
                .map(|v| v.to_string())
                .collect();
            if n.len() == 4 {
                url.push_str(&format!("&bbox={}", n.join(",")));
            }
        }
    }

    let resp = match t.http.get(&url).send().await {
        Ok(r) => r,
        Err(e) => return ToolOutcome::err(format!("photon unreachable: {e}")),
    };
    if !resp.status().is_success() {
        return ToolOutcome::err(format!("photon returned HTTP {}", resp.status()));
    }
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return ToolOutcome::err(format!("photon returned invalid JSON: {e}")),
    };

    let features = body.get("features").and_then(Value::as_array).cloned().unwrap_or_default();
    if features.is_empty() {
        // Not an error — an empty candidate list is a real answer the agent can act
        // on by broadening the query.
        return ToolOutcome::ok(json!({ "candidates": [], "note": "no match" }));
    }

    let candidates: Vec<Value> = features
        .iter()
        .filter_map(|f| {
            let coords = f.pointer("/geometry/coordinates")?.as_array()?;
            let lon = coords.first()?.as_f64()?;
            let lat = coords.get(1)?.as_f64()?;
            let p = f.get("properties")?;
            Some(json!({
                "name": p.get("name").and_then(Value::as_str).unwrap_or(""),
                "type": p.get("osm_value").and_then(Value::as_str).unwrap_or(""),
                "city": p.get("city").and_then(Value::as_str),
                "state": p.get("state").and_then(Value::as_str),
                "country": p.get("country").and_then(Value::as_str),
                "lon": lon,
                "lat": lat,
                // Photon returns extent as [minLon, maxLat, maxLon, minLat]; normalise
                // to the [minLon, minLat, maxLon, maxLat] order every other tool uses.
                "bbox": p.get("extent").and_then(Value::as_array).and_then(|e| {
                    let v: Vec<f64> = e.iter().filter_map(Value::as_f64).collect();
                    if v.len() == 4 { Some(json!([v[0], v[3], v[2], v[1]])) } else { None }
                })
            }))
        })
        .collect();

    let markers = json!({
        "type": "FeatureCollection",
        "features": candidates.iter().map(|c| json!({
            "type": "Feature",
            "geometry": { "type": "Point", "coordinates": [c["lon"], c["lat"]] },
            "properties": { "name": c["name"], "kind": "geocode" }
        })).collect::<Vec<_>>()
    });

    ToolOutcome::ok_with(json!({ "candidates": candidates }), MapPush::Markers(markers))
}
