# Progressive Module Rollouts

This document defines the target rollout model for Fenrir-managed modules once
basic multi-instance runtime and readiness-aware routing are available.

## Goal

Fenrir should move from "restart a running module" to controlled replacement:

1. keep current capacity online
2. start a replacement candidate
3. warm and verify the candidate
4. shift traffic gradually
5. drain and retire the old instance
6. roll back immediately on regression

This is the production-grade direction for request-serving modules.

## Rollout Modes

Fenrir should support four explicit rollout strategies:

- `restart`
  Hardest and cheapest option. Suitable for development or single-instance services.
- `rolling_replace`
  Start a surge instance, wait for readiness, replace one old instance at a time.
- `canary_replace`
  Like `rolling_replace`, but traffic shifts progressively by percentage.
- `worker_handover`
  For queue/background services. New workers warm up first, then acquire work gradually.

## Instance Lifecycle

Runtime instances should be modeled with operational roles rather than only
`primary` and `replica`.

- `active`
  Receives normal traffic or normal work leases.
- `candidate`
  Newly started instance that is not yet promoted.
- `warming`
  Candidate is alive and passing basic readiness, but still under warmup checks.
- `canary`
  Candidate receives a controlled fraction of production traffic.
- `draining`
  Existing instance no longer accepts new traffic and is finishing in-flight work.
- `retired`
  Instance was replaced successfully and is safe to stop.
- `failed`
  Instance failed readiness, warmup or success criteria checks.

## Control Loop

The control loop for HTTP-style modules should be:

1. Detect desired rollout because of config change, version change or manual action.
2. Start `max_surge` candidate instances.
3. Wait until every candidate passes:
   - process alive
   - `/ready`
   - service manifest registration
   - optional warmup timeout window
4. Move candidates from `candidate` to `warming`.
5. Run success checks over a configured interval.
6. If healthy, start traffic shifting:
   - `10 -> 25 -> 50 -> 100`
7. Mark old instance `draining`.
8. Keep old instance serving only in-flight requests until drain timeout expires.
9. Stop old instance and mark candidate `active`.
10. Repeat per instance until rollout is complete.

Rollback should abort immediately when a candidate violates configured success criteria.

## Traffic Shifting

Canary shifting should be policy-driven rather than hardcoded.

- `traffic_steps = [10, 25, 50, 100]`
- `promotion_interval_ms = 30000`
- `rollback_on_regression = true`
- `stickiness = false` by default

Promotion must depend on metrics, not only on time.

## Success Criteria

Fenrir should evaluate promotion against service-level signals.

- error rate
- p95 latency
- retry rate
- queue backlog
- connection saturation
- health/readiness flaps

The minimum policy contract should include:

- `max_error_rate_percent`
- `max_p95_latency_ms`
- `max_retry_rate_percent`
- `max_queue_backlog`

## Request-serving Modules

### `athene-web`

Best rollout style: `canary_replace`

Reasons:

- stateless HTTP delivery
- easy readiness contract
- low risk from partial traffic shifting

Suggested defaults:

- `replicas = 2`
- `traffic_steps = [10, 25, 50, 100]`
- small warmup window

### `athene-api`

Best rollout style: `canary_replace`

Reasons:

- request/response service
- direct gateway exposure
- traffic shifting is operationally valuable

Suggested defaults:

- `replicas = 2`
- strict error-rate and latency thresholds
- canary promotion only with stable upstream gateway metrics

### `athene`

Best rollout style: `rolling_replace`

Reasons:

- backend behavior may be broader and more stateful
- conservative replacement is safer than aggressive canary by default

Suggested defaults:

- `replicas = 2`
- `max_surge = 1`
- `max_unavailable = 0`

### `auth-service`

Best rollout style: `rolling_replace`

Reasons:

- security-sensitive behavior
- higher cost of accidental regressions
- session or side-effect risks should bias toward conservative rollout

Suggested defaults:

- longer warmup
- stricter rollback thresholds
- prefer zero unavailable capacity

## Worker-style Modules

### `notification-service`

Best rollout style: `worker_handover`

Reasons:

- percentage-based request traffic is not the right abstraction
- safe rollout depends on lease handover and idempotency

Worker handover should mean:

1. start candidate workers
2. keep them idle or on a limited lease budget
3. confirm queue processing health
4. gradually increase lease/work share
5. drain old workers

### `auth-service` background workers

If `auth-service` contains delivery or maintenance workers, those should use the
same handover rules instead of pure request-side canary routing.

## Routing Requirements

Fenrir routing should evolve from simple round-robin to policy-aware balancing.

Required capabilities:

- only route to `ready` instances
- route to `canary` instances by configured weight
- remove `draining` instances from new request selection
- optionally support sticky affinity for session-sensitive flows
- retry connect/reset failures against another ready instance when safe

## Warmup Requirements

An instance should not be promoted just because the process is alive.

Warmup may include:

- readiness endpoint
- control-plane registration
- service manifest registration
- connection pool establishment
- cache/bootstrap completion
- stable metrics across the warmup window

## Rollback Rules

Rollback should not mean "restart everything".

Rollback should:

1. stop traffic increase immediately
2. route traffic back to still-healthy active instances
3. remove failed candidates from the eligible pool
4. keep old instances alive until recovery is confirmed
5. record rollout failure reason and stage

## Compatibility Rules

Old and new versions may coexist during replacement. This requires:

- backward/forward compatible APIs
- additive or staged database schema changes
- event compatibility
- no shared exclusive port assumptions

This is especially important for `athene`, `auth-service` and `notification-service`.

## Configuration Model

Fenrir's configuration model should allow per-service rollout policy:

```toml
[modules.runtime.rollout.replacement]
enabled = true
default_strategy = "rolling_replace"
default_max_surge = 1
default_max_unavailable = 0
default_warmup_timeout_ms = 30000
default_promotion_interval_ms = 30000

[modules.services."module:athene-api"]
replicas = 2

[modules.services."module:athene-api".rollout]
strategy = "canary_replace"
max_surge = 1
max_unavailable = 0
warmup_timeout_ms = 30000
drain_timeout_ms = 15000
traffic_steps = [10, 25, 50, 100]
promotion_interval_ms = 30000
rollback_on_regression = true
stickiness = false

[modules.services."module:athene-api".rollout.success_criteria]
max_error_rate_percent = 1.0
max_p95_latency_ms = 500
max_retry_rate_percent = 2.0
```

## Implementation Order

Recommended delivery order:

1. model rollout policy in config
2. introduce instance roles in runtime state
3. add policy-aware routing and weight support
4. add candidate warmup and promotion controller
5. add worker handover semantics
6. add rollback reason capture and rollout event history
7. add kill/chaos/load tests

## Current Status

Already available:

- multi-instance process runtime
- replica reconciliation
- readiness-aware routing to healthy instances
- rolling replace with temporary surge capacity

Still missing for full progressive rollout:

- weighted canary routing
- promotion controller based on success criteria
- worker lease handover controller
- retry/failover routing policy
- rollout event history and regression analytics

## Current Manual Canary Controls

Fenrir now supports weighted canary routing at the routing layer for modules that
use `strategy = "canary_replace"`.

Current operator commands:

```text
modules canary <module-id> status
modules canary <module-id> start [instance-id ...]
modules canary <module-id> set <percent> [instance-id ...]
modules canary <module-id> clear
```

Behavior:

- only ready/running instances participate in routing
- configured canary instance ids receive the requested traffic percentage
- if no explicit instance ids are provided, Fenrir uses non-primary instances as canary candidates
- routing remains round-robin inside the selected stable or canary pool
- `start` uses the first configured `traffic_steps` entry and lets Fenrir promote dynamically
- Fenrir evaluates canary progress during its health-monitor cycle
- on regression, Fenrir rolls back canary routing automatically when `rollback_on_regression = true`

Current automatic promotion uses the configured service diagnostics and runtime metrics:

- `max_error_rate_percent`
- `max_p95_latency_ms`
- `max_retry_rate_percent`
- `max_queue_backlog`

Promotion still depends on configured `promotion_interval_ms` and `traffic_steps`.
