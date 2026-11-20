# ADR: Keep the runtime a supervised modular monolith

**Status:** Accepted for the current NedoBot runtime. **Date:** 2026-11-16.

## Context

NedoBot has durable PostgreSQL-backed jobs and leases, feature-local repositories, one Telegram dispatcher per configured community instance, and a bounded set of long-lived workers. T06–T18 extracted route, feature, delivery, and moderation boundaries incrementally. T20 asks whether a new vertical slice requires changing internals of unrelated features.

## Decision

Keep one supervised process per bot instance and explicit worker registration in `main.rs`. Keep command registration in the existing command enum/adapter. Use feature modules, typed effect ports, bounded queues, PostgreSQL job rows, leases, typed status transitions, and process-level restart for an unexpectedly exited worker. Do not add ECS, a generic actor runtime, global event bus, service locator, or dynamically discovered plugin registry.

## Reasons

- Durable jobs already provide restart recovery and cross-process coordination; another actor persistence model would duplicate state ownership.
- Feature workers have a small fixed topology selected by compile-time features and validated instance config, so runtime actor discovery provides no demonstrated need.
- Explicit startup wiring keeps feature gates and resource ownership reviewable. The integration cost of adding a module remains visible instead of hidden behind a universal framework.
- A worker panic now becomes a process failure, so systemd restores the supervisor and database leases recover abandoned work instead of the application silently degrading.

## Tradeoffs and revisit criteria

Adding a long-lived feature touches `main.rs`, its feature module/repository, typed configuration, observability and tests. Command additions touch the command enum and dispatch adapter. This is accepted while the number of registrations is small and explicit.

Revisit only with evidence of repeated unsafe startup drift, dynamic tenant-specific module loading, substantial worker-count growth causing measurable restart or resource-isolation problems, or a requirement for independent per-worker deployment/scaling. A replacement must preserve PostgreSQL as job source of truth, bounded resource admission, instance scope, shutdown drain, delivery ambiguity semantics, and observability across process restart.
