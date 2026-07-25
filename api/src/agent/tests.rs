//! Agent-loop tests (design spec §13, layer 2).
//!
//! The loop is the riskiest code in the service and is fully testable with zero API
//! calls and zero cost: a mock backend replays canned `tool_use` / `end_turn` /
//! `refusal` / `pause_turn` turns and records the exact `messages` array it was
//! handed. Every invariant in §10 gets a test here.
//!
//! Tool calls deliberately run against an unconfigured PostGIS pool so they fail
//! deterministically. The invariants under test are about how the loop *assembles
//! messages*, which holds identically whether a tool succeeded or failed — and the
//! failure path additionally exercises invariant 3.

use super::*;
use crate::config::Config;
use std::sync::{Arc, Mutex};

fn test_config() -> Config {
    Config {
        photon_url: "http://127.0.0.1:1".into(),
        graphhopper_url: "http://127.0.0.1:1".into(),
        pg_host: "127.0.0.1".into(),
        pg_port: 1,
        pg_user: "x".into(),
        pg_password: "x".into(),
        pg_db: "x".into(),
        anthropic_api_key: "test".into(),
        anthropic_base: "http://127.0.0.1:1".into(),
        model: "claude-opus-5".into(),
        effort: "high".into(),
        task_budget: 20_000,
        max_tokens: 4096,
        max_iterations: 12,
        bind: "127.0.0.1:0".into(),
    }
}

fn test_tools() -> Tools {
    Tools { cfg: test_config(), http: reqwest::Client::new(), pool: None }
}

fn agent_cfg(max_iterations: usize) -> AgentConfig {
    AgentConfig {
        model: "claude-opus-5".into(),
        effort: "high".into(),
        task_budget: 20_000,
        max_tokens: 4096,
        max_iterations,
    }
}

/// Replays a scripted list of turns and records every `messages` array it receives.
struct MockBackend {
    turns: Mutex<std::collections::VecDeque<Turn>>,
    seen: Arc<Mutex<Vec<Value>>>,
}

impl MockBackend {
    fn new(turns: Vec<Turn>) -> (Self, Arc<Mutex<Vec<Value>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self { turns: Mutex::new(turns.into()), seen: seen.clone() },
            seen,
        )
    }
}

impl ModelBackend for MockBackend {
    async fn send(
        &self,
        body: Value,
        _tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Turn, BackendError> {
        self.seen
            .lock()
            .unwrap()
            .push(body.get("messages").cloned().unwrap_or(json!([])));
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| BackendError::Fatal("mock ran out of turns".into()))
    }
}

fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({ "type": "tool_use", "id": id, "name": name, "input": input })
}

fn poi_input() -> Value {
    json!({ "tags": ["natural=volcano"], "bbox": [112.0, -8.0, 113.0, -7.0], "limit": 10 })
}

async fn drain(rx: &mut mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    // Give spawned tool tasks a moment, then drain again.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

async fn run_with(turns: Vec<Turn>, max_iter: usize) -> (Vec<Value>, Vec<AgentEvent>) {
    let (backend, seen) = MockBackend::new(turns);
    let tools = test_tools();
    let (tx, mut rx) = mpsc::channel(256);
    let start = vec![json!({ "role": "user", "content": "plan a trip" })];
    super::run(&backend, &tools, &agent_cfg(max_iter), start, tx).await;
    let events = drain(&mut rx).await;
    let seen = seen.lock().unwrap().clone();
    (seen, events)
}

/// Invariant 1: the assistant's full `content` array is appended, not extracted
/// text. Dropping the tool_use block would leave the next request with a
/// tool_result matching nothing, which the API rejects outright.
#[tokio::test]
async fn appends_full_content_array_including_tool_use() {
    let (seen, _) = run_with(
        vec![
            Turn {
                content: vec![
                    json!({ "type": "text", "text": "Looking for volcanoes." }),
                    tool_use("toolu_1", "search_poi", poi_input()),
                ],
                stop_reason: "tool_use".into(),
            },
            Turn { content: vec![json!({ "type": "text", "text": "Done." })], stop_reason: "end_turn".into() },
        ],
        12,
    )
    .await;

    let second = &seen[1];
    let assistant = &second[1];
    assert_eq!(assistant["role"], "assistant");
    let content = assistant["content"].as_array().expect("content is an array");
    assert_eq!(content.len(), 2, "both blocks preserved, not just the text");
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "tool_use");
    assert_eq!(content[1]["id"], "toolu_1");
}

/// Invariant 2: ALL tool_result blocks go in a SINGLE user message. Splitting them
/// across messages silently teaches the model to stop making parallel tool calls —
/// which is exactly the behaviour we want for "volcanoes AND beaches".
#[tokio::test]
async fn parallel_tool_results_land_in_one_user_message() {
    let (seen, _) = run_with(
        vec![
            Turn {
                content: vec![
                    tool_use("toolu_a", "search_poi", poi_input()),
                    tool_use("toolu_b", "search_poi", poi_input()),
                ],
                stop_reason: "tool_use".into(),
            },
            Turn { content: vec![json!({ "type": "text", "text": "Done." })], stop_reason: "end_turn".into() },
        ],
        12,
    )
    .await;

    let second = seen[1].as_array().unwrap();
    // [user prompt, assistant turn, ONE user message carrying both results]
    assert_eq!(second.len(), 3, "results must not be split across messages");

    let results = second[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let ids: Vec<&str> = results.iter().map(|r| r["tool_use_id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"toolu_a") && ids.contains(&"toolu_b"));
    // Order mirrors the tool_use order even though execution is concurrent.
    assert_eq!(ids, vec!["toolu_a", "toolu_b"]);
}

/// Invariant 3: a failed tool comes back as a tool_result with `is_error: true` —
/// never dropped, never a hard failure. The agent reads it and adapts.
#[tokio::test]
async fn failed_tool_returns_is_error_not_a_dropped_block() {
    let (seen, events) = run_with(
        vec![
            Turn {
                content: vec![tool_use("toolu_1", "search_poi", poi_input())],
                stop_reason: "tool_use".into(),
            },
            Turn { content: vec![json!({ "type": "text", "text": "Adapting." })], stop_reason: "end_turn".into() },
        ],
        12,
    )
    .await;

    let results = seen[1].as_array().unwrap()[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 1, "the failure is reported, not dropped");
    assert_eq!(results[0]["is_error"], true);
    assert_eq!(results[0]["tool_use_id"], "toolu_1");
    // And the loop kept going rather than aborting.
    assert!(matches!(events.last(), Some(AgentEvent::Done)));
}

/// An unknown tool name is still a tool_result, not a crash.
#[tokio::test]
async fn unknown_tool_is_reported_as_an_error_result() {
    let (seen, _) = run_with(
        vec![
            Turn {
                content: vec![tool_use("toolu_1", "teleport", json!({}))],
                stop_reason: "tool_use".into(),
            },
            Turn { content: vec![json!({ "type": "text", "text": "ok" })], stop_reason: "end_turn".into() },
        ],
        12,
    )
    .await;
    let results = seen[1].as_array().unwrap()[2]["content"].as_array().unwrap();
    assert_eq!(results[0]["is_error"], true);
    assert!(results[0]["content"].as_str().unwrap().contains("unknown tool"));
}

/// Invariant 4: check `stop_reason` before touching `content`. On a refusal the
/// content array can be empty — indexing it first would panic.
#[tokio::test]
async fn refusal_with_empty_content_is_surfaced_not_panicked() {
    let (_, events) = run_with(
        vec![Turn { content: vec![], stop_reason: "refusal".into() }],
        12,
    )
    .await;
    assert!(
        matches!(events.last(), Some(AgentEvent::Refused { .. })),
        "expected a refusal event, got {events:?}"
    );
}

/// `pause_turn` appends the partial assistant turn and re-sends. Adding a
/// "continue" user message here would corrupt the server-side resume.
#[tokio::test]
async fn pause_turn_appends_assistant_and_continues() {
    let (seen, events) = run_with(
        vec![
            Turn {
                content: vec![json!({ "type": "text", "text": "partial" })],
                stop_reason: "pause_turn".into(),
            },
            Turn { content: vec![json!({ "type": "text", "text": "finished" })], stop_reason: "end_turn".into() },
        ],
        12,
    )
    .await;

    let second = seen[1].as_array().unwrap();
    assert_eq!(second.len(), 2, "no synthetic user turn was injected");
    assert_eq!(second[1]["role"], "assistant");
    assert!(matches!(events.last(), Some(AgentEvent::Done)));
}

/// The hard iteration cap stops a runaway loop.
#[tokio::test]
async fn hard_iteration_cap_terminates_the_loop() {
    let turns = (0..10)
        .map(|i| Turn {
            content: vec![tool_use(&format!("toolu_{i}"), "search_poi", poi_input())],
            stop_reason: "tool_use".into(),
        })
        .collect();

    let (seen, events) = run_with(turns, 3).await;
    assert_eq!(seen.len(), 3, "stopped at max_iterations");
    match events.last() {
        Some(AgentEvent::Error { message }) => assert!(message.contains("3 iterations")),
        other => panic!("expected an iteration-cap error, got {other:?}"),
    }
}

/// A fatal backend error ends the stream with an error event rather than hanging.
#[tokio::test]
async fn backend_failure_emits_error_event() {
    let (_, events) = run_with(vec![], 12).await;
    assert!(matches!(events.last(), Some(AgentEvent::Error { .. })));
}

/// Geometry reaches the browser as its own event, so the map can draw a leg before
/// the model has finished reasoning about it.
#[tokio::test]
async fn request_body_carries_the_documented_shape() {
    let cfg = agent_cfg(12);
    let body = build_request(&cfg, &[json!({ "role": "user", "content": "hi" })]);

    assert_eq!(body["model"], "claude-opus-5");
    assert_eq!(body["stream"], true);
    // Opus 5 rejects temperature/top_p/top_k outright.
    assert!(body.get("temperature").is_none());
    assert!(body.get("top_p").is_none());
    assert!(body.get("top_k").is_none());
    // Thinking is on by default on Opus 5, but the default display is "omitted",
    // which streams as a silent pause. We opt into summaries explicitly.
    assert_eq!(body["thinking"]["display"], "summarized");
    assert_eq!(body["output_config"]["task_budget"]["type"], "tokens");
    assert_eq!(body["fallbacks"], "default");
    // Caching the system block covers the tool definitions too, since tools render
    // before system.
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["tools"].as_array().unwrap().len(), 4);
}

/// Every tool is declared strict, with additionalProperties:false and each property
/// listed in `required` — that combination is what makes the inputs schema-guaranteed.
#[tokio::test]
async fn every_tool_is_strict_and_closed() {
    for tool in crate::tools::definitions().as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        assert_eq!(tool["strict"], true, "{name} must be strict");
        let schema = &tool["input_schema"];
        assert_eq!(schema["additionalProperties"], false, "{name} must be closed");

        let props: Vec<&str> = schema["properties"].as_object().unwrap().keys().map(|s| s.as_str()).collect();
        let required: Vec<&str> =
            schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        for p in &props {
            assert!(required.contains(p), "{name}.{p} must be listed in required");
        }
    }
}

/// Structured outputs support only a subset of JSON Schema. `minItems`/`maxItems`
/// above 1, and the numeric/string range keywords, are rejected at request time
/// with a 400 — which surfaces as the whole conversation failing, not as a bad
/// tool call. Runtime validation in the tool functions covers these instead.
#[tokio::test]
async fn tool_schemas_avoid_unsupported_json_schema_keywords() {
    const BANNED: [&str; 6] =
        ["minItems", "maxItems", "minimum", "maximum", "minLength", "maxLength"];

    fn walk(v: &Value, path: &str, found: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for (k, child) in map {
                    if BANNED.contains(&k.as_str()) {
                        // minItems/maxItems of 0 or 1 are permitted; anything else is not.
                        let ok = matches!(k.as_str(), "minItems" | "maxItems")
                            && matches!(child.as_u64(), Some(0) | Some(1));
                        if !ok {
                            found.push(format!("{path}.{k}"));
                        }
                    }
                    walk(child, &format!("{path}.{k}"), found);
                }
            }
            Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    walk(child, &format!("{path}[{i}]"), found);
                }
            }
            _ => {}
        }
    }

    let mut found = Vec::new();
    walk(&crate::tools::definitions(), "tools", &mut found);
    assert!(found.is_empty(), "unsupported schema keywords present: {found:?}");
}

/// Strict tool use also rejects `additionalProperties: true` — every object in the
/// tree must be closed. This is why the CustomModel is declared structurally rather
/// than left free-form.
#[tokio::test]
async fn every_object_in_the_schema_tree_is_closed() {
    fn walk(v: &Value, path: &str, open: &mut Vec<String>) {
        if let Value::Object(map) = v {
            let declares_object = map
                .get("type")
                .map(|t| t == "object" || t.as_array().map(|a| a.contains(&json!("object"))).unwrap_or(false))
                .unwrap_or(false);
            if declares_object && map.get("additionalProperties") != Some(&json!(false)) {
                open.push(path.to_string());
            }
            for (k, child) in map {
                walk(child, &format!("{path}.{k}"), open);
            }
        } else if let Value::Array(items) = v {
            for (i, child) in items.iter().enumerate() {
                walk(child, &format!("{path}[{i}]"), open);
            }
        }
    }

    let mut open = Vec::new();
    walk(&crate::tools::definitions(), "tools", &mut open);
    assert!(open.is_empty(), "objects left open: {open:?}");
}

/// The model must send every property, so unused alternatives arrive as null.
/// GraphHopper rejects a rule carrying them, so they are pruned on the way out.
#[tokio::test]
async fn null_alternatives_are_pruned_before_graphhopper_sees_them() {
    use crate::tools::route::{body, prune_nulls};

    // Uses a distance_influence at or above the landmark floor so this test covers
    // pruning alone; the clamp is exercised separately below.
    let from_model = json!({
        "priority": [
            { "if": "road_class == MOTORWAY", "else_if": null, "else": null,
              "multiply_by": 0.05, "limit_to": null }
        ],
        "speed": null,
        "distance_influence": 120
    });

    let pruned = prune_nulls(&from_model);
    assert_eq!(
        pruned,
        json!({
            "priority": [{ "if": "road_class == MOTORWAY", "multiply_by": 0.05 }],
            "distance_influence": 120
        })
    );

    let b = body(&[[112.75, -7.25], [112.63, -7.96]], Some(&from_model));
    assert_eq!(b["custom_model"], pruned);

    // A model where the agent filled in nothing prunes to {}, which GraphHopper
    // rejects — treat it as no preference rather than forwarding an empty object.
    let empty = json!({ "priority": null, "speed": null, "distance_influence": null });
    let b = body(&[[112.75, -7.25], [112.63, -7.96]], Some(&empty));
    assert!(b.get("custom_model").is_none());
}

/// GraphHopper rejects a query-time distance_influence below what LM was prepared
/// against. Observed live: "CustomModel in query can only use distance_influence
/// bigger or equal to 90.0, but was: 40.0". Nothing below the floor is reachable,
/// so we clamp and report rather than burning an agent round trip on the error.
#[tokio::test]
async fn distance_influence_below_the_landmark_floor_is_clamped_and_reported() {
    use crate::tools::route::{normalize_custom_model, MIN_DISTANCE_INFLUENCE};

    let too_low = json!({
        "priority": [{ "if": "road_class == MOTORWAY", "else_if": null, "else": null,
                       "multiply_by": 0.02, "limit_to": null }],
        "speed": null,
        "distance_influence": 40
    });
    let (model, note) = normalize_custom_model(&too_low);
    let model = model.expect("model survives clamping");
    assert_eq!(model["distance_influence"], json!(MIN_DISTANCE_INFLUENCE));
    let note = note.expect("the adjustment is reported back to the agent");
    assert!(note.contains("90"), "note should name the floor: {note}");

    // At or above the floor the model is forwarded untouched, with no note.
    let fine = json!({ "priority": null, "speed": null, "distance_influence": 150 });
    let (model, note) = normalize_custom_model(&fine);
    assert_eq!(model.unwrap()["distance_influence"], json!(150));
    assert!(note.is_none());
}

/// The three non-default /route parameters, asserted at the point they are built.
/// A regression here is silent: elevation would vanish while ascend still reports,
/// and custom models would 400 only for requests that carry one.
#[tokio::test]
async fn route_body_sets_the_three_non_default_parameters() {
    let wp = vec![[112.75, -7.25], [112.63, -7.96]];
    let b = crate::tools::route::body(&wp, None);
    assert_eq!(b["elevation"], true, "3D graph alone returns 2D coordinates");
    assert_eq!(b["ch.disable"], true, "CH cannot serve a runtime custom_model");
    assert_eq!(b["points_encoded"], false, "otherwise geometry is a polyline");

    let cm = json!({ "priority": [{ "if": "road_class == MOTORWAY", "multiply_by": 0.05 }] });
    let b = crate::tools::route::body(&wp, Some(&cm));
    assert_eq!(b["custom_model"], cm);

    // A null custom_model is the schema's "not supplied" encoding — it must not be
    // forwarded, or GraphHopper rejects the request.
    let b = crate::tools::route::body(&wp, Some(&json!(null)));
    assert!(b.get("custom_model").is_none());
}

#[tokio::test]
async fn waypoints_reject_reversed_lat_lon() {
    // -7.25 as a longitude is legal, but 112.75 as a latitude is not — this is the
    // check that catches a [lat, lon] mix-up before it reaches GraphHopper.
    let bad = json!({ "waypoints": [[-7.25, 112.75], [-7.96, 112.63]] });
    let err = crate::tools::route::parse_waypoints(&bad, "waypoints").unwrap_err();
    assert!(err.contains("out of range"), "got: {err}");

    let good = json!({ "waypoints": [[112.75, -7.25], [112.63, -7.96]] });
    assert!(crate::tools::route::parse_waypoints(&good, "waypoints").is_ok());
}

#[tokio::test]
async fn human_duration_formats_hours_and_minutes() {
    assert_eq!(crate::tools::route::human_duration(4_607_274.0), "1h17m");
    assert_eq!(crate::tools::route::human_duration(600_000.0), "10m");
}
