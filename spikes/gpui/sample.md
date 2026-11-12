# Spec: payment retries

The **billing lead** owns this task. It was delegated by the *CEO* on `2026-11-11`
and has a budget of **$4.00**. See the [GPUI source](https://github.com/zed-industries/zed) for the framework.

## Goals

1. Retry a failed charge up to three times.
2. Back off exponentially, starting at `30s`.
   - Jitter each delay by up to 10%.
   - Never retry a card that was *declined*.
3. Record every attempt in the event log.

## Interface

```rust
pub fn next_attempt(prev: Attempt, now: Instant) -> Option<Instant> {
    let delay = BASE * 2u32.pow(prev.number);
    (prev.number < 3).then(|| now + jitter(delay))
}
```

## Budget

| Agent | Role | Spent | Budget |
|---|---|---|---|
| ceo | CEO | $0.42 | $10.00 |
| billing | Lead | $1.17 | $4.00 |
| worker-3 | Worker | $0.08 | $0.50 |

> A held action never runs before it is approved.

---

### Open questions

- Should a retry wake the lead, or only the worker?
- Is `idempotency_key` enough to make a retried charge safe?
