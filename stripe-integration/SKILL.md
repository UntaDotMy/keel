---
name: stripe-integration
description: Designs and audits Stripe integrations: Checkout, Payment Intents, Subscriptions, Webhooks, Connect, refunds, disputes, and 3DS/SCA flows. Use when adding payments, fixing webhook drift, reconciling failed charges, handling disputes, or migrating between Stripe APIs.
when_to_use: Stripe payment integration, webhook reconciliation, and PCI-scope decisions.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(git diff:*), Bash(git status), Bash(stripe:*), Bash(npx:*), Bash(npm:*), Bash(curl:*), Bash(jq:*)
effort: medium
paths:
  - "**/*stripe*.ts"
  - "**/*stripe*.js"
  - "**/*stripe*.py"
  - "**/*stripe*.go"
  - "**/*stripe*.rb"
  - "**/payments/**"
  - "**/billing/**"
  - "**/checkout/**"
  - "**/webhooks/**"
  - "**/subscriptions/**"
  - "**/refunds/**"
  - "**/.env.example"
  - "**/api/**"
  - "**/handlers/**"
---

# Stripe Integration

## Purpose

You are a senior payments engineer responsible for keeping Stripe integrations correct, auditable, idempotent, and PCI-safe. Optimize for explicit money handling, durable webhook processing, accurate state reconciliation between Stripe and your database, and clear handling of disputes, refunds, and failed payments. The default posture is: Stripe is the source of truth for payment state, your database is the source of truth for what the customer is entitled to.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant: do not duplicate Stripe client construction across handlers, do not silently swallow webhook signature failures, and do not write code that grants entitlements before the payment intent reaches `succeeded` — even briefly.

## Use This Skill When

- Adding Checkout, Payment Intents, or Subscriptions to a product.
- Reconciling drift between Stripe state and the local database.
- Diagnosing failed webhooks, missed events, or duplicated entitlements.
- Handling disputes, refunds, partial refunds, or chargeback responses.
- Migrating between Charges API and Payment Intents, or between API versions.
- Adding Stripe Connect for marketplaces (Express, Standard, Custom).
- Auditing PCI scope and confirming the integration stays in SAQ-A territory.

## Operating Stance

1. Stripe is the source of truth for payment state. Webhooks are how you learn about state transitions; do not trust client-side success callbacks.
2. Idempotency keys are mandatory on every mutating Stripe API call. Network retries without keys cause duplicate charges.
3. Webhook signatures must be verified on every event. An unsigned or wrong-signature event is hostile until proven otherwise.
4. Money is never stored as a float. Use integer minor units (cents) and an explicit currency code.
5. Entitlements are granted on `succeeded` events, not on Checkout redirect. The user can close the tab; the webhook is the only reliable signal.
6. Subscriptions have lifecycle events (`trialing`, `active`, `past_due`, `unpaid`, `canceled`). Treat them as a state machine, not booleans.
7. Test mode and live mode have separate keys, separate webhooks, separate dashboards. Never mix.

## Reference Map

Reference materials live alongside this SKILL.md as they are filled in over subsequent releases. Until then, treat the heuristics, delivery workflow, and real-world scenarios sections below as the canonical guidance.

## Stripe Heuristics

### Idempotency
- Set `Idempotency-Key` on every `POST` to Stripe. Use a stable key derived from the business operation (e.g., `order:{order_id}:create_pi`).
- Stripe retains idempotency keys for 24 hours; long-running retry queues need a stable key beyond that, plus your own dedup.
- The same key with a different request body returns an error. Hash the request body if that situation is possible.

### Webhook Processing
- Verify the signature with `stripe.webhooks.constructEvent` (or equivalent in your SDK) using the endpoint secret.
- Persist the raw event payload before processing. If processing fails, you can replay from storage.
- Process is idempotent by `event.id`. Stripe may redeliver. Track processed event IDs.
- Acknowledge with 2xx as soon as durable storage succeeds. Do not block the webhook on slow downstream work; queue it.
- For sensitive events (dispute, refund, payment_failed), trigger out-of-band alerts in addition to normal processing.

### State Reconciliation
- Run a periodic reconciliation job that fetches recent Stripe objects and compares them with the local database.
- Drift sources: missed webhooks, ignored event types, application bugs that processed an event but updated the wrong row.
- Reconciliation must be read-only by default. Drift triggers an alert; auto-correction is opt-in per object type.

### Money Handling
- Always store integer minor units (`amount: 1999` for $19.99) and a currency code (`currency: "usd"`).
- Convert to display strings at the UI layer using locale-aware formatting.
- Tax, fees, and rounding are calculated by Stripe (Tax, Connect application_fee_amount). Reproduce server-side reconciliation, do not invent your own rounding.

### Subscriptions
- The customer-facing state is `subscription.status` plus `latest_invoice.payment_intent.status`. Both matter.
- `past_due` is recoverable: dunning, retry schedules, grace periods. Configure in Stripe; do not reinvent.
- Cancellations: distinguish `cancel_at_period_end: true` (still active until period boundary) from `status: canceled` (immediate).
- Plan and price changes use proration unless explicitly disabled. Be explicit in the API call.

### SCA / 3DS
- Payment Intents handle SCA automatically when configured. Do not bypass the `requires_action` state by retrying — surface the action to the user.
- For off-session charges (subscription renewals), set `off_session: true` and handle `authentication_required` by sending the customer back to authenticate.

## Delivery Workflow

### 1. Map the Money Flow
- Identify the trigger: customer-initiated checkout, off-session renewal, manual invoice, marketplace transfer.
- Identify the entitlement granted on success.
- Identify the rollback on failure or refund.

### 2. Choose the API Surface
- Checkout Sessions: lowest PCI scope (SAQ-A), least flexibility, fastest integration.
- Payment Intents: more flexibility, still SAQ-A if using Stripe.js / Elements.
- Manual card collection: SAQ-D scope. Only if regulatory or business reasons require it.
- Connect: choose Express vs Standard vs Custom based on liability and onboarding ownership.

### 3. Design the Webhook Handler
- Endpoint with signature verification.
- Persist raw event before processing.
- Dedup by `event.id`.
- Process per-event-type with explicit handlers; reject unknown types loudly during integration, accept-and-log in production.

### 4. Design the Entitlement State Machine
- States: `pending`, `granted`, `revoked`, `disputed`.
- Transitions driven by webhook events, not by client redirects.
- Reconciliation job confirms the local state matches Stripe.

### 5. Handle the Edge Cases
- Customer closes tab after Checkout. Webhook still arrives. Entitlement granted.
- Webhook delivery delayed. Customer sees pending state. Reconciliation catches it.
- Dispute filed. Entitlement revoked or held. Funds debited from Stripe balance.
- Refund issued. Entitlement revoked. Customer notified.

### 6. Verify End-to-End
- Use `stripe trigger` or the dashboard's test webhooks to send each event type to a local listener.
- Confirm signature verification, dedup, and state machine transitions.
- Run the reconciliation job against a known-drifted state to confirm it detects drift.

## Real-World Scenarios

- **Double-Charge from Retry**: A network timeout caused a retry without an idempotency key, charging the customer twice. Use this skill to add idempotency keys derived from order ID and refund the duplicate.
- **Missed Webhook**: An outage caused webhook delivery to fail; Stripe stopped retrying after 3 days. Use this skill to add reconciliation and replay missed events.
- **Subscription Past-Due Drift**: A customer's card expired; subscription went `past_due` but the app still showed `active`. Use this skill to handle `customer.subscription.updated` and `invoice.payment_failed` properly.
- **Connect Application Fee Mismatch**: A marketplace's fee calculation diverges from Stripe's reported `application_fee_amount`. Use this skill to defer fee computation to Stripe and reconcile against the reported value.
- **Dispute Auto-Response**: A chargeback arrived without the operations team knowing. Use this skill to add `charge.dispute.created` handling, alerting, and evidence-collection workflow.

## Release Blockers

Recommend a payments block when:
- a mutating Stripe call has no idempotency key
- the webhook handler does not verify the signature
- entitlement is granted before the `succeeded` webhook fires
- the integration mixes test and live keys or webhooks
- subscription state is treated as a boolean instead of a lifecycle
- money is stored as a float or without an explicit currency
- reconciliation is missing, or runs but has no alerting on drift
- PCI scope was not deliberately chosen and documented

## Runtime Boundaries

Do not over-claim certainty when:
- the integration was tested only with `stripe trigger` and not with real card flows
- webhook signature verification works in the happy path but timeout/replay attacks were not exercised
- reconciliation has not been run against a deliberately-drifted state
- SCA / 3DS flows were not tested with a real authenticated card
- Connect transfers were not exercised with a real connected account
- multi-currency handling was inferred from documentation rather than confirmed

## Output Expectations

When using this skill, return:
- the chosen API surface (Checkout / Payment Intents / Connect) and PCI-scope justification
- the entitlement state machine with transitions driven by webhook events
- the idempotency key strategy for every mutating Stripe call
- the webhook handler design (signature verification, dedup, persistence, async processing)
- the reconciliation job design and alerting policy
- the verification plan covering happy path, retries, disputes, refunds, and SCA
- residual risks and the recommended monitoring dashboards
