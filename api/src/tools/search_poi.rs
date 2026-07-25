use super::{MapPush, ToolOutcome, Tools};
use serde_json::{json, Value};

/// PostGIS POI search by OSM tag inside a bbox.
///
/// The query shape is fixed and parameterised — the agent supplies tag key/value
/// pairs as data, never SQL. `geom && ST_MakeEnvelope(...)` is what hits the GiST
/// index that `make db` builds; tests/smoke/test_postgis.sh asserts that plan.
pub async fn run(t: &Tools, input: &Value) -> ToolOutcome {
    let pool = match &t.pool {
        Some(p) => p,
        None => return ToolOutcome::err("postgis is not configured"),
    };

    let tags: Vec<(String, String)> = match input.get("tags").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|s| s.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect(),
        _ => return ToolOutcome::err("`tags` must be a non-empty array of 'key=value' strings"),
    };
    if tags.is_empty() {
        return ToolOutcome::err("no tag parsed — each entry must look like 'natural=volcano'");
    }

    let bbox: Vec<f64> = input
        .get("bbox")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    if bbox.len() != 4 {
        return ToolOutcome::err("`bbox` must be [min_lon, min_lat, max_lon, max_lat]");
    }

    let limit = input.get("limit").and_then(Value::as_i64).unwrap_or(50).clamp(1, 200);

    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(format!("postgis pool error: {e}")),
    };

    // One OR-ed condition per tag, all bound as parameters.
    let mut conds = Vec::new();
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
    params.push(Box::new(bbox[0]));
    params.push(Box::new(bbox[1]));
    params.push(Box::new(bbox[2]));
    params.push(Box::new(bbox[3]));
    for (k, v) in &tags {
        params.push(Box::new(k.clone()));
        params.push(Box::new(v.clone()));
        let ki = params.len() - 1;
        let vi = params.len();
        conds.push(format!("(tags->>${ki} = ${vi})"));
    }
    params.push(Box::new(limit));
    let limit_idx = params.len();

    let sql = format!(
        "SELECT osm_id, name, tags, ST_X(geom) AS lon, ST_Y(geom) AS lat \
         FROM osm_poi \
         WHERE geom && ST_MakeEnvelope($1, $2, $3, $4, 4326) \
           AND ({}) \
           AND name IS NOT NULL \
         LIMIT ${}",
        conds.join(" OR "),
        limit_idx
    );

    let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();

    let rows = match client.query(sql.as_str(), &refs[..]).await {
        Ok(r) => r,
        Err(e) => return ToolOutcome::err(format!("postgis query failed: {e}")),
    };

    let pois: Vec<Value> = rows
        .iter()
        .map(|r| {
            let tags: Value = r.get::<_, Value>("tags");
            json!({
                "osm_id": r.get::<_, i64>("osm_id"),
                "name": r.get::<_, Option<String>>("name"),
                "lon": r.get::<_, f64>("lon"),
                "lat": r.get::<_, f64>("lat"),
                // Only the tags the agent reasons about — the raw jsonb can be large
                // and most of it is metadata the model never uses.
                "tags": json!({
                    "natural": tags.get("natural"),
                    "tourism": tags.get("tourism"),
                    "amenity": tags.get("amenity"),
                    "historic": tags.get("historic"),
                    "leisure": tags.get("leisure"),
                }),
            })
        })
        .collect();

    let markers = json!({
        "type": "FeatureCollection",
        "features": pois.iter().map(|p| json!({
            "type": "Feature",
            "geometry": { "type": "Point", "coordinates": [p["lon"], p["lat"]] },
            "properties": { "name": p["name"], "kind": "poi" }
        })).collect::<Vec<_>>()
    });

    ToolOutcome::ok_with(
        json!({ "count": pois.len(), "pois": pois }),
        MapPush::Markers(markers),
    )
}
