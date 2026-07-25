use super::{AgentEvent, BackendError, ModelBackend, Turn};
use futures::StreamExt;
use rand::Rng;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

/// `task_budget` caps the whole agentic loop rather than one response — the model
/// sees a countdown and wraps up gracefully instead of being cut off.
/// `fallbacks: "default"` matters because Opus 5's safety classifiers can decline
/// with `stop_reason: "refusal"` on an HTTP 200; without a fallback the request just
/// stops. Each beta gates one of those two features.
const BETAS: &str = "task-budgets-2026-03-13,server-side-fallback-2026-07-01";
const API_VERSION: &str = "2023-06-01";
const MAX_ATTEMPTS: u32 = 5;

pub struct AnthropicBackend {
    pub http: reqwest::Client,
    pub api_key: String,
    pub base_url: String,
}

impl ModelBackend for AnthropicBackend {
    async fn send(
        &self,
        body: Value,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Turn, BackendError> {
        if self.api_key.is_empty() {
            return Err(BackendError::Fatal(
                "ANTHROPIC_API_KEY is not set on the server".into(),
            ));
        }

        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));

        // Invariant 5: retry 429 and 5xx with exponential backoff + jitter. The
        // official SDKs do this for you; raw HTTP means we own it. Only the
        // pre-stream failure is retried — once bytes have reached the client,
        // retrying would duplicate emitted text.
        let mut attempt = 0u32;
        let resp = loop {
            attempt += 1;
            let sent = self
                .http
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header("anthropic-beta", BETAS)
                .json(&body)
                .send()
                .await;

            match sent {
                Ok(r) if r.status().is_success() => break r,
                Ok(r) => {
                    let status = r.status();
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    if !retryable || attempt >= MAX_ATTEMPTS {
                        let detail = r.text().await.unwrap_or_default();
                        let detail = detail.chars().take(400).collect::<String>();
                        return Err(BackendError::Fatal(format!(
                            "anthropic returned HTTP {status}: {detail}"
                        )));
                    }
                    // Honour Retry-After when the server sends one.
                    let hinted = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok());
                    sleep_backoff(attempt, hinted).await;
                }
                Err(e) => {
                    if attempt >= MAX_ATTEMPTS {
                        return Err(BackendError::Fatal(format!("anthropic unreachable: {e}")));
                    }
                    sleep_backoff(attempt, None).await;
                }
            }
        };

        parse_stream(resp, tx).await
    }
}

async fn sleep_backoff(attempt: u32, hinted_secs: Option<u64>) {
    let base = hinted_secs
        .map(|s| Duration::from_secs(s.min(30)))
        .unwrap_or_else(|| Duration::from_millis(500 * 2u64.pow(attempt.min(5))));
    let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..250));
    tokio::time::sleep(base + jitter).await;
}

#[derive(Default)]
struct BlockAcc {
    kind: String,
    base: Value,
    text: String,
    thinking: String,
    signature: String,
    json_buf: String,
}

impl BlockAcc {
    fn finish(self) -> Value {
        match self.kind.as_str() {
            "text" => json!({ "type": "text", "text": self.text }),
            "thinking" => {
                // The signature must be echoed back verbatim on the next request —
                // the API rejects a modified thinking block.
                let mut v = json!({ "type": "thinking", "thinking": self.thinking });
                if !self.signature.is_empty() {
                    v["signature"] = json!(self.signature);
                }
                v
            }
            "tool_use" => {
                let input: Value =
                    serde_json::from_str(&self.json_buf).unwrap_or_else(|_| json!({}));
                json!({
                    "type": "tool_use",
                    "id": self.base.get("id").cloned().unwrap_or(json!("")),
                    "name": self.base.get("name").cloned().unwrap_or(json!("")),
                    "input": input,
                })
            }
            // redacted_thinking, fallback markers, server-tool blocks: pass through
            // untouched so the next request replays them exactly as received.
            _ => self.base,
        }
    }
}

async fn parse_stream(
    resp: reqwest::Response,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<Turn, BackendError> {
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut blocks: Vec<Option<BlockAcc>> = Vec::new();
    let mut stop_reason = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| BackendError::Fatal(format!("stream broke: {e}")))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        // SSE frames are separated by a blank line; a frame may span chunks.
        while let Some(pos) = buf.find("\n\n") {
            let frame = buf[..pos].to_string();
            buf.drain(..pos + 2);

            let Some(data) = frame
                .lines()
                .find_map(|l| l.strip_prefix("data:").map(str::trim))
            else {
                continue;
            };
            let Ok(ev) = serde_json::from_str::<Value>(data) else { continue };

            match ev.get("type").and_then(Value::as_str).unwrap_or("") {
                "content_block_start" => {
                    let idx = ev.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let cb = ev.get("content_block").cloned().unwrap_or(json!({}));
                    let kind = cb.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                    while blocks.len() <= idx {
                        blocks.push(None);
                    }
                    blocks[idx] = Some(BlockAcc { kind, base: cb, ..Default::default() });
                }
                "content_block_delta" => {
                    let idx = ev.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let Some(Some(acc)) = blocks.get_mut(idx) else { continue };
                    let delta = ev.get("delta").cloned().unwrap_or(json!({}));
                    match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text_delta" => {
                            let s = delta.get("text").and_then(Value::as_str).unwrap_or("");
                            acc.text.push_str(s);
                            let _ = tx.send(AgentEvent::Text { text: s.to_string() }).await;
                        }
                        "thinking_delta" => {
                            let s = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                            acc.thinking.push_str(s);
                            let _ = tx.send(AgentEvent::Thinking { text: s.to_string() }).await;
                        }
                        "signature_delta" => {
                            acc.signature
                                .push_str(delta.get("signature").and_then(Value::as_str).unwrap_or(""));
                        }
                        "input_json_delta" => {
                            acc.json_buf.push_str(
                                delta.get("partial_json").and_then(Value::as_str).unwrap_or(""),
                            );
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    if let Some(sr) = ev.pointer("/delta/stop_reason").and_then(Value::as_str) {
                        stop_reason = sr.to_string();
                    }
                }
                "error" => {
                    let msg = ev
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown stream error");
                    return Err(BackendError::Fatal(msg.to_string()));
                }
                _ => {}
            }
        }
    }

    let content: Vec<Value> = blocks.into_iter().flatten().map(BlockAcc::finish).collect();
    if stop_reason.is_empty() {
        return Err(BackendError::Fatal("stream ended without a stop_reason".into()));
    }
    Ok(Turn { content, stop_reason })
}
