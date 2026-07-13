//! Retry with exponential backoff for the LLM HTTP calls. Wraps the *send +
//! status* phase of a request — before any streaming tokens are read — so a
//! transient failure (network drop, 429, 5xx) is retried without ever
//! double-emitting streamed output.

use std::time::Duration;

use crate::types::LlmError;

/// Backoff/retry policy. Defaults are overridable via env vars so a flaky local
/// server can be tuned without a rebuild:
/// `FLOWS_LLM_MAX_RETRIES`, `FLOWS_LLM_RETRY_BASE_MS`, `FLOWS_LLM_RETRY_MAX_MS`.
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 4,
            // The upstream (model-swap proxy) waits ~5s on a failed backend before
            // it's ready, so the first retry waits at least 6s (6/12/24/30s).
            base_delay_ms: 6_000,
            max_delay_ms: 30_000,
        }
    }
}

impl RetryConfig {
    /// Default policy with any env-var overrides applied.
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Some(v) = env_u64("FLOWS_LLM_MAX_RETRIES") {
            c.max_retries = v as u32;
        }
        if let Some(v) = env_u64("FLOWS_LLM_RETRY_BASE_MS") {
            c.base_delay_ms = v;
        }
        if let Some(v) = env_u64("FLOWS_LLM_RETRY_MAX_MS") {
            c.max_delay_ms = v;
        }
        c
    }

    /// Backoff before the given attempt (1-based): `base * 2^(attempt-1)`, capped,
    /// plus up to 25% jitter to avoid synchronized retries.
    fn delay(&self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(20);
        let exp = self.base_delay_ms.saturating_mul(1u64 << shift);
        let capped = exp.min(self.max_delay_ms);
        let jitter = jitter_seed() % (capped / 4 + 1);
        Duration::from_millis(capped + jitter)
    }
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse().ok()
}

/// A cheap, dependency-free jitter source (sub-second nanos of the wall clock).
fn jitter_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
}

/// Retryable transient HTTP statuses: request timeout, too-early, rate limit, 5xx.
fn is_retryable_status(code: u16) -> bool {
    code == 408 || code == 425 || code == 429 || (500..=599).contains(&code)
}

/// Send a request with retries. `build` produces a fresh [`reqwest::RequestBuilder`]
/// per attempt (the body must be rebuildable). Retries on network errors and
/// retryable statuses with exponential backoff; returns the final [`reqwest::Response`]
/// (of any status — the caller still handles non-success bodies) or the last
/// network error once retries are exhausted.
pub async fn send_with_retry<F>(cfg: &RetryConfig, build: F) -> Result<reqwest::Response, LlmError>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt = 0u32;
    loop {
        match build().send().await {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if resp.status().is_success() || !is_retryable_status(code) {
                    return Ok(resp);
                }
                // Retryable error status: back off and retry, else return it so
                // the caller reads the body and builds the final Api error.
                if attempt >= cfg.max_retries {
                    return Ok(resp);
                }
                attempt += 1;
                tokio::time::sleep(cfg.delay(attempt)).await;
            }
            Err(e) => {
                if attempt >= cfg.max_retries {
                    return Err(LlmError::Http(e.to_string()));
                }
                attempt += 1;
                tokio::time::sleep(cfg.delay(attempt)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_status_classification() {
        for c in [408, 425, 429, 500, 502, 503, 504] {
            assert!(is_retryable_status(c), "{c} should retry");
        }
        for c in [200, 201, 400, 401, 403, 404, 422] {
            assert!(!is_retryable_status(c), "{c} should not retry");
        }
    }

    #[test]
    fn backoff_grows_and_caps() {
        let c = RetryConfig {
            max_retries: 10,
            base_delay_ms: 100,
            max_delay_ms: 1000,
        };
        // 100, 200, 400, 800, then capped at 1000 (+ up to 25% jitter).
        assert!(c.delay(1).as_millis() >= 100 && c.delay(1).as_millis() < 200);
        assert!(c.delay(2).as_millis() >= 200 && c.delay(2).as_millis() < 300);
        assert!(c.delay(4).as_millis() >= 800 && c.delay(4).as_millis() < 1050);
        // Deep attempts stay capped (+jitter ≤ 25%).
        assert!(c.delay(20).as_millis() <= 1250);
    }

    #[test]
    fn default_first_wait_at_least_6s() {
        let c = RetryConfig::default();
        // First retry must exceed the upstream's ~5s recovery window.
        assert!(c.delay(1).as_millis() >= 6_000, "first wait must be >= 6s");
    }

    #[test]
    fn env_overrides() {
        std::env::set_var("FLOWS_LLM_MAX_RETRIES", "7");
        let c = RetryConfig::from_env();
        std::env::remove_var("FLOWS_LLM_MAX_RETRIES");
        assert_eq!(c.max_retries, 7);
    }
}
