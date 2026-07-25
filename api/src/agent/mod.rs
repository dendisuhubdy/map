pub mod anthropic;

#[cfg(test)]
mod tests;

use crate::tools::{MapPush, Tools};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Structured events streamed to the browser. The map updates *as* the agent works
/// rather than after, which is why this is a typed event stream and not a plain
/// text stream (design spec §10).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Text { text: String },
    Thinking { text: String },
    ToolStart { name: String, input: Value },
    ToolEnd { name: String, ok: bool },
    Geometry { geojson: Value },
    Markers { geojson: Value },
    Refused { message: String },
    Error { message: String },
    Done,
}

#[derive(Debug)]
pub struct Turn {
    /// The assistant's full `content` array, verbatim.
    pub content: Vec<Value>,
    pub stop_reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("{0}")]
    Fatal(String),
}

pub trait ModelBackend {
    fn send(
        &self,
        body: Value,
        tx: mpsc::Sender<AgentEvent>,
    ) -> impl std::future::Future<Output = Result<Turn, BackendError>> + Send;
}

pub struct AgentConfig {
    pub model: String,
    pub effort: String,
    pub task_budget: u32,
    pub max_tokens: u32,
    pub max_iterations: usize,
}

pub const SYSTEM_PROMPT: &str = r#"You are a trip planner for Indonesia. You turn a description of a trip into a real, routed itinerary using open map data.

You have four tools: `geocode` resolves names to coordinates, `search_poi` finds places by OpenStreetMap tag inside a box, `route` computes driving routes, and `elevation_profile` reports climb. All coverage is Indonesia only — say so plainly if asked for anywhere else.

How to work:
- Resolve place names before using them. Never invent coordinates; a plausible-looking guess is worse than a geocode call.
- Call tools in parallel when the calls are independent. Looking for volcanoes AND beaches is two `search_poi` calls in one turn, not two turns.
- Honour hard constraints exactly. If the user caps driving at 4 hours a day, check each leg with `route` and re-plan the ones that exceed it rather than reporting a leg you know is over.
- Express soft preferences through `route`'s `custom_model`, not by picking waypoints you hope are scenic. "Twisty" is a curvature rule; "avoid highways" is a road_class rule; "no tolls" is a toll rule. Combine them.
- A leg that fails to route is information, not a dead end — pick a different waypoint and say what you changed.

Coordinates are always [longitude, latitude], in that order.

Answer with the itinerary itself: the days, the stops, the driving time per leg. Keep the prose tight — the map shows the route, so don't narrate the geometry. State distances and durations from tool results, never estimated. If you could not satisfy a constraint, say which one and why."#;

/// Assemble the request body.
///
/// `cache_control` sits on the last system block so the system prompt and the four
/// tool definitions are cached together — the loop resends the whole conversation
/// every turn, so this is the difference between paying for that prefix once versus
/// once per tool call. Tools render before system, so one breakpoint covers both.
pub fn build_request(cfg: &AgentConfig, messages: &[Value]) -> Value {
    json!({
        "model": cfg.model,
        "max_tokens": cfg.max_tokens,
        "stream": true,
        "system": [{
            "type": "text",
            "text": SYSTEM_PROMPT,
            "cache_control": { "type": "ephemeral" }
        }],
        "thinking": { "type": "adaptive", "display": "summarized" },
        "output_config": {
            "effort": cfg.effort,
            "task_budget": { "type": "tokens", "total": cfg.task_budget }
        },
        "fallbacks": "default",
        "tools": crate::tools::definitions(),
        "messages": messages,
    })
}

/// The agent loop. Conversation in, event stream out.
pub async fn run<B: ModelBackend>(
    backend: &B,
    tools: &Tools,
    cfg: &AgentConfig,
    mut messages: Vec<Value>,
    tx: mpsc::Sender<AgentEvent>,
) {
    for _ in 0..cfg.max_iterations {
        let body = build_request(cfg, &messages);

        let turn = match backend.send(body, tx.clone()).await {
            Ok(t) => t,
            Err(BackendError::Fatal(m)) => {
                let _ = tx.send(AgentEvent::Error { message: m }).await;
                return;
            }
        };

        // Invariant 4: check stop_reason before touching content. On a refusal the
        // content array may be empty, so anything that indexes it would panic.
        match turn.stop_reason.as_str() {
            "refusal" => {
                let _ = tx
                    .send(AgentEvent::Refused {
                        message: "The model declined this request.".into(),
                    })
                    .await;
                return;
            }
            "end_turn" | "stop_sequence" => {
                let _ = tx.send(AgentEvent::Done).await;
                return;
            }
            "max_tokens" => {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: "Response hit the token ceiling before finishing.".into(),
                    })
                    .await;
                return;
            }
            "pause_turn" => {
                // A server-side tool paused mid-turn. Append the partial assistant
                // turn and re-send; the server resumes where it left off. Adding a
                // "continue" user message here would corrupt the resume.
                messages.push(json!({ "role": "assistant", "content": turn.content }));
                continue;
            }
            "tool_use" => {}
            other => {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: format!("unexpected stop_reason '{other}'"),
                    })
                    .await;
                return;
            }
        }

        // Invariant 1: append the full content array, not extracted text. Dropping
        // the tool_use blocks would leave the next request with tool_results that
        // match nothing, which the API rejects.
        messages.push(json!({ "role": "assistant", "content": turn.content.clone() }));

        let calls: Vec<(String, String, Value)> = turn
            .content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .filter_map(|b| {
                Some((
                    b.get("id")?.as_str()?.to_string(),
                    b.get("name")?.as_str()?.to_string(),
                    b.get("input").cloned().unwrap_or(json!({})),
                ))
            })
            .collect();

        if calls.is_empty() {
            let _ = tx
                .send(AgentEvent::Error {
                    message: "model asked for tools but sent no tool_use block".into(),
                })
                .await;
            return;
        }

        // Execute concurrently — "volcanoes AND beaches" should cost one round trip,
        // not two.
        let mut set = tokio::task::JoinSet::new();
        for (idx, (id, name, input)) in calls.into_iter().enumerate() {
            let tools = tools.clone();
            let tx = tx.clone();
            set.spawn(async move {
                let _ = tx
                    .send(AgentEvent::ToolStart { name: name.clone(), input: input.clone() })
                    .await;
                let outcome = tools.dispatch(&name, &input).await;
                let _ = tx
                    .send(AgentEvent::ToolEnd { name: name.clone(), ok: !outcome.is_error })
                    .await;
                // Push geometry to the map immediately, before the model has even
                // seen the result.
                match &outcome.map_push {
                    Some(MapPush::Geometry(g)) => {
                        let _ = tx.send(AgentEvent::Geometry { geojson: g.clone() }).await;
                    }
                    Some(MapPush::Markers(m)) => {
                        let _ = tx.send(AgentEvent::Markers { geojson: m.clone() }).await;
                    }
                    None => {}
                }
                (idx, id, outcome)
            });
        }

        let mut results = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(r) => results.push(r),
                Err(e) => {
                    let _ = tx
                        .send(AgentEvent::Error { message: format!("tool task panicked: {e}") })
                        .await;
                    return;
                }
            }
        }
        // Concurrency reorders completions; tool_result order should still mirror the
        // tool_use order for readability.
        results.sort_by_key(|(idx, _, _)| *idx);

        // Invariant 2: ALL tool_result blocks go in a single user message. Splitting
        // them across messages teaches the model to stop making parallel calls —
        // exactly the behaviour we want to keep.
        // Invariant 3: a failed tool is a result with is_error, never a dropped block.
        let blocks: Vec<Value> = results
            .into_iter()
            .map(|(_, id, outcome)| {
                json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": outcome.content,
                    "is_error": outcome.is_error,
                })
            })
            .collect();

        messages.push(json!({ "role": "user", "content": blocks }));
    }

    let _ = tx
        .send(AgentEvent::Error {
            message: format!("stopped after {} iterations without finishing", cfg.max_iterations),
        })
        .await;
}
