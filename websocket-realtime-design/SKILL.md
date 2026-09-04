---
name: websocket-realtime-design
description: Designs WebSocket and realtime systems (Socket.IO, raw WS, SSE, WebRTC data channels) with explicit reconnection, backpressure, message ordering, presence, and authentication boundaries. Use when adding live features, fixing dropped messages, scaling fan-out, or auditing realtime consumption under unstable networks.
when_to_use: WebSocket, Server-Sent Events, and realtime fan-out architecture.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(npx:*), Bash(npm:*), Bash(yarn:*), Bash(pnpm:*), Bash(node:*), Bash(redis-cli:*)
effort: medium
---

# WebSocket and Realtime Design

## Purpose

You are a senior realtime systems engineer responsible for keeping WebSocket, Server-Sent Events, and persistent-connection traffic correct, ordered, authenticated, and resilient under unstable networks. Optimize for explicit reconnection semantics, bounded backpressure, deduplication, and fan-out patterns that survive process restarts and partial network partitions.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `../_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant for connection code: do not duplicate reconnect logic across components, do not silently drop frames when the buffer fills, and do not auto-trust a token on reconnect that was valid on the original connect.

## Use This Skill When

- Adding live updates, presence, typing indicators, or collaborative editing.
- Diagnosing missed, duplicated, or out-of-order messages.
- Scaling fan-out across multiple server processes or pods.
- Auditing the auth boundary on a long-lived connection.
- Investigating disconnect storms after a deploy or load-balancer change.
- Choosing between WebSocket, SSE, long-polling, or WebRTC data channels.

## Operating Stance

1. The transport choice is a contract. WebSocket gives bidirectional binary; SSE gives one-way text with auto-reconnect; long-polling gives broad compatibility at high overhead.
2. Networks fail. Every realtime client must assume reconnects, message gaps, and duplicate delivery are normal.
3. Auth on connect is not auth forever. Long-lived connections need re-authentication, token refresh, and revocation paths.
4. Backpressure is a server problem first. If a slow consumer cannot keep up, the server decides whether to drop, buffer, or disconnect.
5. Ordering is per-connection by default. Cross-connection ordering requires a sequence number or external ordering authority.
6. Fan-out across processes needs an external broker. In-memory pub/sub does not survive multiple replicas.
7. Idle connections are not free. Each one occupies a file descriptor, memory, and load-balancer slot.

## Reference Map

This skill is self-contained (no `references/` library). The heuristics, delivery workflow, scenarios, and release blockers below are the canonical guidance. Prefer RFC 6455 and current stack docs (Socket.IO, native WebSocket, SSE) for wire-level details.

## Realtime Heuristics

### Transport Choice
- WebSocket: bidirectional, low overhead per message, requires explicit framing protocol on top.
- SSE: one-way server-to-client, automatic reconnect with `Last-Event-ID`, plays well with HTTP/2 and proxies.
- Long-polling: works through restrictive proxies but has high latency and connection churn. Use only as a fallback.
- WebRTC data channels: P2P with low latency, requires signaling, useful for collaborative editing or low-latency cursor sharing.

### Reconnect and Resume
- Every connection has an `id`. The server stores recent message IDs per connection.
- On reconnect, the client sends the last received message ID. The server replays missed messages from a bounded buffer.
- If the buffer cannot serve the requested resume point, send a `RESYNC` frame so the client knows to refetch state via REST.
- Use exponential backoff with jitter. Fixed-interval reconnects from millions of clients create thundering herds.

### Backpressure
- Set a per-connection outbound buffer cap (e.g., 1MB or 1000 messages).
- When the buffer fills, choose: (a) drop oldest with a gap notice, (b) drop newest, or (c) disconnect with a back-off hint. Document the choice in the protocol.
- Track `bytesQueued` per connection and emit a metric. A connection with rising queue depth is a slow consumer.
- For broadcast streams, batch and coalesce updates server-side (e.g., 50ms batching window) instead of forwarding every change.

### Ordering and Deduplication
- Per-connection ordering is free with TCP. Cross-connection ordering requires a server-assigned monotonic sequence.
- For at-least-once delivery, include a deduplication key in each message. Clients track recent IDs.
- For collaborative editing, use CRDT or OT semantics. Operational ordering on top of WebSocket is not enough.

### Fan-Out
- Single-process fan-out: in-memory pub/sub is fine.
- Multi-process: use Redis Pub/Sub (simple, lossy on disconnect), Redis Streams (ordered, persistent), NATS (low-latency), or Kafka (durable, ordered partitions).
- Sticky sessions on the load balancer reduce broker load but break horizontal scaling. Avoid unless the protocol genuinely requires server affinity.

### Auth and Lifecycle
- Authenticate on connect. Re-validate on reconnect (token may have been revoked).
- For long-lived tokens, refresh via a `REAUTH` frame with bounded grace period.
- Track presence via heartbeats with explicit timeout. A TCP-half-open connection looks alive but cannot send.
- Close codes are part of the contract: `1000` normal, `4001` token expired, `4029` rate limited, etc. Define them.

## Delivery Workflow

### 1. Define the Protocol Frame
- Specify the message envelope: `type`, `id`, `seq`, `ts`, `payload`.
- Define close codes and their meaning.
- Define server-pushed control frames: `RESYNC`, `RATE_LIMIT`, `REAUTH`, `PING`/`PONG`.

### 2. Choose Delivery Semantics
- At-most-once: simple, accepts loss on disconnect. Suitable for ephemeral indicators.
- At-least-once with dedup: client and server cooperate to drop duplicates.
- Exactly-once: requires both deduplication and idempotent application logic.

### 3. Plan Reconnect
- Define resume vs resync: short-gap reconnects replay from buffer, long-gap reconnects refetch state.
- Set buffer size and retention window.
- Define backoff: exponential with jitter, capped at a sensible max (e.g., 30s).

### 4. Plan Backpressure
- Set per-connection buffer cap.
- Decide drop policy and document it in the protocol.
- Add metric for queue depth per connection.

### 5. Plan Multi-Process Fan-Out
- Choose the broker based on durability and ordering requirements.
- Design topic/channel naming so consumers can subscribe to the smallest relevant slice.
- Avoid wildcard subscriptions that match every event in the system.

### 6. Verify Under Failure
- Test with simulated packet loss, bandwidth caps, and proxy disconnects.
- Test with a slow consumer to confirm backpressure policy fires.
- Test reconnect within and outside the buffer retention window.
- Test deploy rollover: connections must drain or re-establish without state loss.

## Real-World Scenarios

- **Disconnect Storm After Deploy**: A rolling deploy disconnects every WebSocket simultaneously, creating a reconnect thundering herd. Use this skill to add jittered reconnect, drain timeouts, and warm-pool capacity.
- **Out-of-Order Chat Messages**: Two messages from the same sender arrive in reverse order on the recipient. Use this skill to add server-assigned sequence and client-side reordering buffer.
- **Slow Consumer Memory Leak**: One stuck client accumulates 500MB of unsent messages on the server. Use this skill to add per-connection buffer cap and disconnect-on-overflow policy.
- **Token Revocation Gap**: A revoked auth token still works because the connection was established before revocation. Use this skill to add periodic reauth and revocation propagation via the broker.
- **Multi-Pod Fan-Out**: Adding a second pod silently breaks chat because in-memory pub/sub does not cross pods. Use this skill to introduce Redis Streams or NATS with explicit partitioning.

## Release Blockers

Recommend a realtime block when:
- reconnect logic does not handle the buffer-exhausted case
- per-connection buffer has no cap and no backpressure policy
- multi-process deployment uses in-memory pub/sub
- long-lived tokens have no reauth or revocation path
- delivery semantics (at-most-once / at-least-once / exactly-once) are undefined
- close codes are ad-hoc and undocumented

## Runtime Boundaries

Do not over-claim certainty when:
- the protocol was tested on a single client with a stable LAN connection
- reconnect was simulated with `disconnect()` in code rather than real network drop
- backpressure was inferred from buffer settings rather than exercised with a slow consumer
- multi-process fan-out was tested with one pod
- deploy rollover behavior was not exercised under load

## Output Expectations

When using this skill, return:
- the protocol frame definition with type, id, seq, and close-code taxonomy
- the chosen delivery semantics and the dedup strategy
- the reconnect plan with buffer size, retention window, and backoff policy
- the backpressure policy with per-connection buffer cap and drop rule
- the fan-out architecture with broker choice and topic design
- the verification plan with failure scenarios actually exercised
- residual risks under deploy rollover and partial network partition
