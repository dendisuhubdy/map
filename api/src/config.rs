use std::env;

/// Every value here comes from the environment — the service is 12-factor and
/// carries no host paths or baked-in endpoints (design spec, decision 3).
#[derive(Clone, Debug)]
pub struct Config {
    pub photon_url: String,
    pub graphhopper_url: String,
    pub pg_host: String,
    pub pg_port: u16,
    pub pg_user: String,
    pub pg_password: String,
    pub pg_db: String,
    pub anthropic_api_key: String,
    pub anthropic_base: String,
    pub model: String,
    pub effort: String,
    pub task_budget: u32,
    pub max_tokens: u32,
    pub max_iterations: usize,
    pub bind: String,
}

fn var(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let anthropic_api_key = env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        if anthropic_api_key.is_empty() {
            // Not fatal: the tool endpoints and /healthz still work without it, which
            // keeps the container useful for smoke-testing the data services. Only
            // /api/chat needs the key, and it returns a clear error when unset.
            tracing::warn!("ANTHROPIC_API_KEY is unset — /api/chat will return 503");
        }
        Ok(Self {
            photon_url: var("PHOTON_URL", "http://photon:2322"),
            graphhopper_url: var("GRAPHHOPPER_URL", "http://graphhopper:8989"),
            pg_host: var("POSTGRES_HOST", "postgis"),
            pg_port: var("POSTGRES_PORT", "5432").parse().unwrap_or(5432),
            pg_user: var("POSTGRES_USER", "map"),
            pg_password: var("POSTGRES_PASSWORD", ""),
            pg_db: var("POSTGRES_DB", "map"),
            anthropic_api_key,
            anthropic_base: var("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
            model: var("AGENT_MODEL", "claude-opus-5"),
            effort: var("AGENT_EFFORT", "high"),
            task_budget: var("AGENT_TASK_BUDGET", "120000").parse().unwrap_or(120_000),
            max_tokens: var("AGENT_MAX_TOKENS", "64000").parse().unwrap_or(64_000),
            max_iterations: var("AGENT_MAX_ITERATIONS", "12").parse().unwrap_or(12),
            bind: var("BIND_ADDR", "0.0.0.0:8000"),
        })
    }
}
