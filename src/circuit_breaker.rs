//! Circuit breaker for API resilience.
//!
//! After sustained failures, stops making requests for a cooldown period
//! instead of hammering a failing API. Follows the standard three-state model:
//! Closed (normal) → Open (fail-fast) → HalfOpen (probe).

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

const DEFAULT_FAILURE_THRESHOLD: u32 = 5;
const DEFAULT_COOLDOWN_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Closed,
    Open,
    HalfOpen,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Closed => write!(f, "Closed"),
            State::Open => write!(f, "Open"),
            State::HalfOpen => write!(f, "HalfOpen"),
        }
    }
}

struct Inner {
    state: State,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

/// Circuit breaker that fails fast after sustained API failures.
///
/// - **Closed**: Requests flow normally. Consecutive failures are tracked.
/// - **Open**: After `failure_threshold` consecutive failures, all requests
///   fail immediately for `cooldown` duration.
/// - **HalfOpen**: After cooldown, one probe request is allowed. Success
///   closes the circuit; failure reopens it.
///
/// 404 responses should NOT be recorded as failures — they are valid
/// application responses, not infrastructure problems.
pub struct CircuitBreaker {
    inner: Mutex<Inner>,
    failure_threshold: u32,
    cooldown: Duration,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self::with_config(
            DEFAULT_FAILURE_THRESHOLD,
            Duration::from_secs(DEFAULT_COOLDOWN_SECS),
        )
    }

    pub fn with_config(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: State::Closed,
                consecutive_failures: 0,
                opened_at: None,
            }),
            failure_threshold,
            cooldown,
        }
    }

    /// Returns `true` if a request should be allowed to proceed.
    /// Transitions Open → HalfOpen when cooldown has elapsed.
    pub fn allow_request(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.state {
            State::Closed => true,
            State::Open => {
                if let Some(opened_at) = inner.opened_at {
                    if opened_at.elapsed() >= self.cooldown {
                        debug!("Circuit breaker: cooldown elapsed, transitioning Open -> HalfOpen");
                        inner.state = State::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            State::HalfOpen => true,
        }
    }

    pub fn record_success(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.state != State::Closed {
            debug!(
                "Circuit breaker: success, transitioning {} -> Closed",
                inner.state
            );
        }
        inner.consecutive_failures = 0;
        inner.state = State::Closed;
        inner.opened_at = None;
    }

    /// Record a failed request. Do NOT call this for 404 responses.
    pub fn record_failure(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.consecutive_failures += 1;

        match inner.state {
            State::HalfOpen => {
                warn!("Circuit breaker: probe failed, reopening circuit");
                inner.state = State::Open;
                inner.opened_at = Some(Instant::now());
            }
            State::Closed => {
                if inner.consecutive_failures >= self.failure_threshold {
                    warn!(
                        "Circuit breaker: {} consecutive failures, opening circuit (cooldown: {}s)",
                        inner.consecutive_failures,
                        self.cooldown.as_secs()
                    );
                    inner.state = State::Open;
                    inner.opened_at = Some(Instant::now());
                }
            }
            State::Open => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed_and_allows_requests() {
        let cb = CircuitBreaker::new();
        assert!(cb.allow_request());
    }

    #[test]
    fn stays_closed_below_threshold() {
        let cb = CircuitBreaker::with_config(3, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        assert!(cb.allow_request());
    }

    #[test]
    fn opens_after_threshold_reached() {
        let cb = CircuitBreaker::with_config(3, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.allow_request());
    }

    #[test]
    fn success_resets_failure_count() {
        let cb = CircuitBreaker::with_config(3, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.allow_request());
    }

    #[test]
    fn transitions_to_half_open_after_cooldown() {
        let cb = CircuitBreaker::with_config(2, Duration::from_millis(10));
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.allow_request());

        std::thread::sleep(Duration::from_millis(15));
        assert!(cb.allow_request());
    }

    #[test]
    fn half_open_success_closes_circuit() {
        let cb = CircuitBreaker::with_config(2, Duration::from_millis(10));
        cb.record_failure();
        cb.record_failure();

        std::thread::sleep(Duration::from_millis(15));
        assert!(cb.allow_request()); // HalfOpen
        cb.record_success();
        assert!(cb.allow_request()); // Closed
    }

    #[test]
    fn half_open_failure_reopens_circuit() {
        let cb = CircuitBreaker::with_config(2, Duration::from_millis(10));
        cb.record_failure();
        cb.record_failure();

        std::thread::sleep(Duration::from_millis(15));
        assert!(cb.allow_request()); // HalfOpen
        cb.record_failure();
        assert!(!cb.allow_request()); // Open again
    }

    #[test]
    fn multiple_successes_after_failures_keep_circuit_closed() {
        let cb = CircuitBreaker::with_config(3, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        cb.record_success();
        cb.record_success();
        assert!(cb.allow_request());
    }

    #[test]
    fn open_circuit_blocks_multiple_checks() {
        let cb = CircuitBreaker::with_config(2, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.allow_request());
        assert!(!cb.allow_request());
        assert!(!cb.allow_request());
    }

    #[test]
    fn threshold_of_one_opens_immediately() {
        let cb = CircuitBreaker::with_config(1, Duration::from_secs(30));
        cb.record_failure();
        assert!(!cb.allow_request());
    }
}
