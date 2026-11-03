# AccessPilot — Payments Control Plane: Architecture & Build Blueprint

Nov 3, 2026 · @Riya Rathod

## 1. TL;DR: what you submit

**Submit AccessPilot as a payments control plane for AI-agent spend, not as an agent-paywall demo.** It is a Rust service that sits between an AI agent and Juspay's open-source payment switch, [Hyperswitch](https://github.com/juspay/hyperswitch), and it proves with printed numbers that it never overspends, never double-charges, never unlocks content before verified payment, and fails closed on every unknown.

Why this framing: the Juspay backend JD is about *concise expression of complex payment logic*, *functional programming*, *reliability at scale*, and *self-service / low-code* tooling. Your original concept already has the right instincts (budget caps, idempotency, ledger). This blueprint turns each instinct into something a Juspay engineer can test in five minutes.

**What changes versus the working document**

- The agent and the LLM become the *least* important part. The deterministic core is the product; the LLM is an untrusted planner behind it.
- The payment adapter targets Hyperswitch's free sandbox (Juspay's own stack), with a simulated PSP as fallback and for fault injection.
- The policy rules become a small typed DSL that a non-programmer can edit (matches the JD's low-code / self-service line).
- Every correctness claim gets a test that prints a number.

### Headline claims (targets: claim only what your harness actually prints)

| # | Claim | How it is tested | Target |
| --- | --- | --- | --- |
| 1 | Zero overspend under concurrency | 5,000 concurrent purchase attempts against one ₹50 cap | captured total ≤ cap; 0 violations |
| 2 | Exactly-once charge | 1,000 concurrent retries of one idempotency key | 1 PSP charge; 999 replayed responses |
| 3 | Fail-closed | Fault matrix: PSP timeout, 5xx, lost/duplicate/forged webhook, DB crash mid-flow, price drift | 0 unauthorized grants |
| 4 | Policy decision latency | Criterion benchmark, 20-rule policy, in-process | p99 under 200 µs |
| 5 | Reserve-and-authorize path | Load test on local Postgres, PSP mocked | p99 under 25 ms; 1,000+ reservations/s across tasks |
| 6 | Invariants hold on random histories | proptest, 100,000 generated event sequences | 0 invariant failures |
| 7 | Reconciliation catches drift | 50 injected ledger-vs-PSP mismatches | 100% detected |

The targets are deliberately set at what a single laptop-class Rust + Postgres setup can plausibly reach. If a measured number misses, report the real number and say what bounds it; honest numbers read better to reviewers than inflated ones.

### Submission package

1. GitHub repo with a README that opens with the architecture diagram and the printed benchmark table.
2. This document, trimmed, as `ARCHITECTURE.md`.
3. A 3-minute screen recording of the demo script in section 15.
4. A live URL on free hosting if time allows; the recording matters more.

## 2. What the Juspay JD actually asks for

**The JD is not a list of tools; it is a list of engineering instincts: express payment logic concisely, think functionally, make reliability and scale provable, and build self-service tooling.** The official [DEV-BE02 page](https://juspay.io/careers/DEV-BE02) describes four Centres of Excellence (SDK, Backend, Data Science, Infrastructure). The Backend and Data Science lines are where this project aims.

| JD signal (from the posting) | What a reviewer will look for | Where AccessPilot answers it |
| --- | --- | --- |
| "Concise expression of complex payment logic" | Rules as data, not nested if-statements | Typed policy DSL (JSON) compiled to an AST and evaluated by one pure function |
| "Biggest FP company with entire business logic in FP" ([FP role page](https://juspay.in/sde-functional-programming)) | Pure core, algebraic types, illegal states unrepresentable | `ap-core` crate: no I/O, enums for every state, total functions, no `unwrap` |
| "Reliability and performance problems at large scale", 99.999% availability goal | Exactly-once semantics, back-pressure, numbers | Reservation + idempotency design, load and chaos harness with printed results |
| "Low code & no code platforms", "self-service customization" | Non-programmers can change behaviour safely | Policy editor with dry-run against past ledger and shadow mode before activation |
| "Intelligent traffic routing, self-healing systems, anomaly detection" | Data-driven choice among providers, automatic recovery | Provider router scored on price and observed success rate, circuit breaker, spend-anomaly detector |
| "UPI, integrations, authentication" | Clean integration boundaries | `PaymentProvider` trait; Hyperswitch is one adapter; webhook signature verification |
| "Merchants of all sizes with zero issues" | Multi-tenancy, isolation | Tenant-scoped budgets, API keys, and ledger partitions |

**Caveat on languages.** The official page names no language. A third-party [job-board listing](https://jobs.weekday.works/juspay-software-development-engineer-backend-intern) of the same role lists Haskell, Java, API and functional programming, and the FP role page names Haskell and PureScript. Treat Haskell as the house language and functional style as the interview theme; this blueprint therefore writes the core in Rust in a strictly functional style (Hyperswitch itself is Rust), and section 4 explains that choice.

**What this project does not replace.** I could not open any interview write-ups, so I have no verified view of the rounds. Keep some hours for data-structures practice; the project earns the conversation, not the offer.

## 3. How Juspay works, and what we deliberately mirror

**Juspay is a payments exchange: one integration for merchants that routes each transaction across many processors and banks, with the business logic written in Haskell and the open-source switch, Hyperswitch, written in Rust.** Figures below come from Juspay's own pages and engineering blog, opened for this document.

- Juspay's [engineering write-up](https://dev.to/hyperswitchio/scaling-to-125-million-transactions-per-day-juspays-engineering-principles-2bj1) cites 125 million transactions per day, peaks of 5,000 TPS, processing under 100 ms, and 99.99% uptime. It credits Haskell's strong typing with pre-empting failures, and describes a Redis-backed cache absorbing real-time load that is later drained to the database.
- Its release tooling replays recorded production payloads (ART), shifts traffic in stages starting at 1% (Autopilot), and A/B-tests the new deployment against the stable one.
- The [Hyperswitch repository](https://github.com/juspay/hyperswitch) lists a Rust app server (routing, retries, vaulting, observability), intelligent routing to the processor with the highest predicted auth rate, a decision engine, a 100+ connector library, revenue recovery (smart retries) and 2-way / 3-way reconciliation.

| Juspay / Hyperswitch idea | AccessPilot mirror | Why it plays well in an interview |
| --- | --- | --- |
| One API, many processors | `PaymentProvider` trait; Hyperswitch adapter plus simulated PSP | Shows you understand orchestration, not one gateway |
| Intelligent routing by predicted auth rate | Offer / provider router ranking by price and observed success (EWMA), with a stored explanation | Same idea at small scale, explainable |
| Revenue recovery, smart retries | Retry only error classes that are safe to retry, always with the same idempotency key, behind a circuit breaker | Shows you know retries are where double-charges come from |
| 2-way reconciliation | Reconciler job: ledger vs Hyperswitch payment status, flags and heals drift | Directly echoes a Hyperswitch module |
| Strong typing prevents failures | Every lifecycle is an enum with a total transition function | The FP argument made concrete |
| Staged rollout and A/B (Autopilot) | Policy versions run in **shadow mode** first; decisions are compared, none enforced | A cheap echo of their release discipline |
| Redis shock absorber in front of the database | Deliberately **not** used for budget reservations: money correctness beats raw speed, so reservations use Postgres atomicity. A Redis fast path is listed as a stretch with the trade-off written down | A reasoned deviation shows more judgement than copying |

One honest scoping line for the README: AccessPilot reuses ideas from a payment switch; it is not a PCI-scoped card-handling system and never touches raw card data outside Hyperswitch's sandbox.

## 4. Free-tier stack decisions

**Total cost is ₹0 and no card is needed: build in Antigravity with Claude Opus 4.6 on the hard crates, run the agent on Groq with a scripted fallback, pay through Hyperswitch's sandbox, and measure everything on local Docker Postgres.** Free-tier limits change often; every limit below is a third-party or vendor-page figure as of early November 2026, so check each at signup.

### 4.1 Language and framework

| Option | Juspay signal | Speed to a working system in hours | Verdict |
| --- | --- | --- | --- |
| Rust (Axum, sqlx, tokio) | High: Hyperswitch is Rust; enums and traits give the FP argument | Medium; cold build takes minutes, then incremental builds are fast | **Chosen.** Keep `ap-core` dependency-light so it compiles in seconds |
| Haskell (Servant, Persistent) | Highest: the house language | Slowest for you today (my judgement): long first dependency build, fewer AI-assist examples | Only if you already write Haskell daily |
| Python / TypeScript | Low for this role | Fastest | Not recommended; it hides the FP and reliability story |

Design rules that make Rust read as functional: pure functions over `&State -> Decision`, `enum` for every lifecycle, newtypes for money and IDs, `Result` everywhere, `#![deny(clippy::unwrap_used, clippy::panic)]` in the core, no global mutable state.

### 4.2 Models for building (Antigravity, free tier)

A third-party [tracker](https://mvalentsev.github.io/awesome-free-ai-coding/providers/antigravity/) (probed 2 Nov 2026) lists gemini-3.8-flash, claude-opus-4.6, claude-sonnet-4.6 and gpt-oss-120b on the free tier, with weekly allowances that are not published and are separate for Gemini and for Claude/GPT. Claude 5.5 needs a paid plan. Because the Claude allowance is the scarce one, spend it where mistakes cost most.

| Model | Spend it on |
| --- | --- |
| claude-opus-4.6 | `ap-core` (types, policy evaluator, state machines), the reservation and idempotency SQL, property tests. Do these first while quota is fresh |
| claude-sonnet-4.6 | Axum handlers, Hyperswitch adapter, webhook verification, fault-injection PSP |
| gemini-3.8-flash | Dashboard, README, seed data, benchmark scripts, fixtures, repetitive refactors |
| gpt-oss-120b | Backup when the Claude allowance runs out |

### 4.3 Models at runtime (the agent's planner)

The LLM only proposes *which resource to request*; it never sees secrets and never decides a payment. All free endpoints are OpenAI-compatible except Gemini's, which is partly so. Figures are from OpenRouter's own [comparison post](https://openrouter.ai/blog/tutorials/free-llm-apis-compared/) (a vendor with a stake) and move frequently.

| Priority | Provider and model | Quoted free limits | Notes |
| --- | --- | --- | --- |
| 1 | Groq, Llama 3.3 70B | about 30 RPM, 1,000 requests/day | Fast, no card, not used for training |
| 2 | Gemini API (AI Studio), Flash | about 5–15 RPM, 20–1,500/day | Prompts may be used for training outside EU/UK/EEA: send only synthetic demo data |
| 3 | Cerebras, Llama 3.3 70B | about 30 RPM, \~1M tokens/day | Token-based quota |
| 4 | OpenRouter free models | about 20 RPM, 50/day | Last resort |
| 0 | **Scripted planner** (no LLM) | Unlimited | Used in CI, benchmarks and as the demo's safety net |

I could not verify tool-calling quality on these free endpoints. Treat every LLM reply as untrusted text: require JSON that matches a schema, validate it in Rust, and on any mismatch fall back to the scripted planner. That is itself a fail-closed behaviour worth showing.

### 4.4 Payments: Juspay's own free option

- **Hyperswitch Cloud sandbox** (Juspay's open-source switch): sign in at [app.hyperswitch.io](https://app.hyperswitch.io), add the **Dummy Connector**, create an API key. The [API reference](https://api-reference.hyperswitch.io/introduction) gives the sandbox base URL `https://sandbox.hyperswitch.io`, secret keys prefixed `snd_` and publishable keys `pk_snd_`. The [test-credentials page](https://docs.hyperswitch.io/integrations/connectors-integrations/payment-processor-capabilities/payment-methods-setup/test-credentials) lists success, decline and insufficient-funds test cards; check there which cards drive each Dummy Connector outcome before scripting the demo.
- **Self-hosted Hyperswitch** via Docker (`scripts/setup.sh` in the repository) is the offline fallback if the sandbox is slow on demo day.
- **Simulated PSP** inside the repo implements the same trait and injects faults (timeouts, 5xx, delayed or duplicated webhooks). It is what the chaos tests use, because you cannot make the real sandbox misbehave on demand.
- Juspay's own merchant gateway (Express Checkout) needs merchant onboarding; I did not find a self-serve free sandbox, so do not depend on it.

### 4.5 Data and hosting

| Need | Choice | Caveat |
| --- | --- | --- |
| Database for development and every benchmark | PostgreSQL 16 in Docker Compose | Numbers from your machine are the honest ones; say the CPU and RAM |
| Hosted database for a live link (optional) | A free hosted Postgres such as Neon or Supabase | Not verified here; confirm limits and idle-pause behaviour at signup |
| Hosted API and dashboard (optional) | Render free web service | Per a Sept 2026 [roundup](https://flaviocopes.com/hosting-free-tiers.md): 512 MB, under one CPU, spins down after 15 minutes idle, about a minute to wake; open the URL just before judging |
| Avoid | Fly.io (card required, trial only), Railway (one-off trial credit) | Neither is free long-term per the same roundup |

## 7. Policy DSL and engine

**Spending rules are data: a small typed JSON language, parsed into an AST and judged by one pure function, `decide(policy, context) -> Decision`.** This is the piece that answers the JD's "concise expression of complex payment logic" and "low-code, self-service" lines, and it is the easiest part to demonstrate in an interview.

### 7.1 Types (in `ap-core`, no I/O)

```rust
pub enum Autonomy { AskEveryTime, WithinRules, NoPaidAccess }
pub enum Verdict  { Allow, RequireApproval, Deny }

pub enum Expr {
    All(Vec<Expr>), Any(Vec<Expr>), Not(Box<Expr>),
    Cmp(Field, Op, Value),          // amount_paise <= 2000
    In(Field, Vec<Value>),          // provider in ["news-a", "data-b"]
}
pub struct Rule   { id: RuleId, when: Expr, then: Verdict, reason: String }
pub struct Policy { version: u32, autonomy: Autonomy, task_cap: Money,
                    per_purchase_cap: Money, rules: Vec<Rule> }

pub struct Decision { verdict: Verdict, trace: Vec<RuleHit>, policy_version: u32 }
pub fn decide(p: &Policy, c: &DecisionContext) -> Decision   // total, pure
```

### 7.2 Evaluation order (fail-closed by construction)

1. **Hard invariants first, not editable by users:** currency mismatch → Deny; price not fully known (tax or fee unknown) → RequireApproval; expected total above remaining budget or per-purchase cap → Deny; autonomy `NoPaidAccess` and price above zero → Deny.
2. **User rules next, order-independent:** every matching rule contributes its verdict; the most restrictive wins (Deny, then RequireApproval, then Allow).
3. **Default is Deny** when no rule says Allow.
4. `AskEveryTime` autonomy downgrades any Allow to RequireApproval.

Order-independence matters: it means a user editing rules in a form cannot change behaviour by dragging a row, and it gives a clean property test.

### 7.3 Example policy

```json
{
  "version": 3,
  "autonomy": "within_rules",
  "task_cap": {"amount_minor": 5000, "currency": "INR"},
  "per_purchase_cap": {"amount_minor": 2000, "currency": "INR"},
  "rules": [
    {"id": "allow-trusted", "then": "allow",
     "when": {"all": [{"provider": {"in": ["news-a", "data-b"]}},
                      {"amount_minor": {"lte": 2000}}]},
     "reason": "trusted provider under the per-purchase cap"},
    {"id": "approve-unknown", "then": "require_approval",
     "when": {"not": {"provider": {"in": ["news-a", "data-b", "api-c"]}}},
     "reason": "provider not on the allowlist"},
    {"id": "no-subscriptions", "then": "deny",
     "when": {"category": {"eq": "subscription"}},
     "reason": "subscriptions are never autonomous"}
  ]
}
```

Amounts are integer minor units (paise). No floats anywhere in the money path.

### 7.4 Self-service features (the low-code story)

- **Rule builder:** a form in the dashboard that emits this JSON; the JSON stays visible and editable.
- **Validation on load:** unknown field, wrong value type, or currency mismatch rejects the whole policy. A policy that does not parse is never partially applied.
- **Dry run:** replay a candidate policy against the last N recorded offers and show a diff (offer, old verdict, new verdict). No side effects.
- **Shadow mode:** version N+1 is evaluated beside version N on live traffic; N is enforced, differences are logged. Activation is one click once the diff is empty or understood. This mirrors Juspay's staged-rollout habit at small scale.
- **Every decision stores** the policy version and the rule trace, rendered as a sentence: "Denied: expected total ₹40.00 exceeds remaining budget ₹25.00 (hard rule H3)."

### 7.5 Property tests for the evaluator

| Property | Meaning |
| --- | --- |
| Permutation invariance | Shuffling `rules` never changes the verdict |
| Monotonic in price | Raising the amount never turns Deny into Allow |
| Budget safety | If expected total exceeds remaining budget, the verdict is never Allow |
| Unknown is never Allow | Unknown price, unknown provider, or unknown currency cannot yield Allow |
| Round trip | parse(serialize(policy)) equals policy |

## 8. Budget reservation and idempotency: the correctness core

**The cap is enforced by one atomic database statement that reserves money before the payment provider is called, so "check, then pay" races cannot exist.** Idempotency then guarantees that retries replay a stored answer instead of charging again. If you can explain this section cold, you have the interview.

### 8.1 Rules of the money path

- Money is `i64` minor units plus a currency enum. No floats, no strings.
- A database `CHECK (reserved_minor + captured_minor <= cap_minor)` is the last line of defence, independent of application code.
- Never hold a database transaction open across a network call to the provider.
- State change and its ledger event are written in the same transaction, so the ledger can never disagree with state.
- Unknown outcome means "assume the money is spent": the reservation stays held until the truth is known.

### 8.2 Atomic reservation

```sql
UPDATE budgets
   SET reserved_minor = reserved_minor + $amount, version = version + 1
 WHERE task_id = $task AND currency = $currency
   AND reserved_minor + captured_minor + $amount <= cap_minor
RETURNING reserved_minor, captured_minor;
-- 0 rows returned => insufficient budget => Deny, no provider call
```

Under Postgres's default isolation, concurrent `UPDATE`s on the same row queue on the row lock, and each waiter re-checks its `WHERE` clause against the latest committed row. That is why 5,000 simultaneous attempts against a ₹50 cap cannot exceed it: the arithmetic is inside the statement.

### 8.3 Purchase flow (write-ahead intent, then provider, then settle)

1. **Claim the idempotency key.** `INSERT INTO idempotency_keys ... ON CONFLICT DO NOTHING`. Inserted: you own the request. Conflict with the same request hash and a completed status: return the stored response. Conflict with a different hash: reject (key reuse with a different payload). Conflict while in progress: return 409 with `Retry-After`.
2. **Transaction 1 (commit fast).** Run the policy decision, reserve the budget, insert the purchase as `RESERVED`, append ledger event `reserved`.
3. **Call the provider with a deterministic payment ID** derived from the idempotency key (UUIDv5 of tenant and key). Before creating, retrieve by that ID; if it exists, adopt it. Correctness therefore does not depend on whether the provider deduplicates; I could not confirm Hyperswitch's duplicate-ID behaviour from its public docs, so verify it in the first ten minutes (section 15).
4. **Transaction 2, by outcome.**
   - Succeeded and verified by a server-side retrieve: reservation becomes captured, ledger `captured`, then and only then issue the access grant.
   - Definitively failed: release the reservation, ledger `released`.
   - Timeout or unknown: purchase becomes `PENDING_UNKNOWN`, reservation stays held, no grant.
5. **Converge.** A webhook or the reconciler resolves `PENDING_UNKNOWN`; the webhook body is never trusted on its own, it only triggers a retrieve of the payment.
6. **Store the response** against the idempotency key so every later retry gets the identical answer.

### 8.4 Why not a Redis fast path

Juspay's blog describes a cache layer absorbing real-time load before draining to the database. For a spend cap that trade is wrong in a first version: a cache that loses a write loses money-correctness. State this in the README as a deliberate choice, and list the Redis-plus-Lua reservation with background drain as the scale-out design you would measure next.

### 8.5 Grants

An access grant has `UNIQUE (purchase_id)`, a scope, and an expiry. Issuing a grant is a database write in the same transaction that marks the purchase captured, so "grant exists without a verified payment" is a state the schema cannot hold: a foreign key plus a check on purchase status enforce it.

### 8.6 Hot-row honesty

One budget row per task serializes purchases within a task. That is correct and cheap for agent workloads. Benchmark two shapes and report both: 5,000 attempts on one task (worst case) and 5,000 attempts spread over 500 tasks (typical).

## 9. Hyperswitch adapter, webhooks, reconciliation

**All provider-specific behaviour lives behind one trait, and nothing the provider says is believed until AccessPilot has fetched the payment itself and matched amount, currency and ID.** Hyperswitch is the real adapter; the simulated PSP implements the same trait so faults can be injected on demand.

### 9.1 The trait

```rust
#[async_trait]
pub trait PaymentProvider: Send + Sync {
    async fn charge(&self, req: ChargeRequest) -> Result<ProviderPayment, ProviderError>;
    async fn retrieve(&self, provider_payment_id: &str) -> Result<Option<ProviderPayment>, ProviderError>;
    async fn refund(&self, req: RefundRequest) -> Result<ProviderRefund, ProviderError>;
    fn verify_webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<WebhookTrigger, WebhookError>;
}

pub enum ProviderStatus { Succeeded, Failed(FailureClass), Processing, ActionRequired, Unknown }
pub enum FailureClass   { Declined, InsufficientFunds, Timeout, ProviderDown, InvalidRequest }
// FailureClass::is_safe_to_retry() is a pure function with its own tests
```

### 9.2 Hyperswitch mapping

| Concern | Design | Status |
| --- | --- | --- |
| Base URL and keys | `https://sandbox.hyperswitch.io`, secret key `snd_…` in server env only | From the [API reference](https://api-reference.hyperswitch.io/introduction) |
| Auth header | Expected `api-key`; confirm against the OpenAPI spec | Unverified, check first |
| Create and confirm | `POST /payments` with `amount` (minor units), `currency`, `confirm: true`, `capture_method: automatic`, our deterministic `payment_id`, `customer_id`, `metadata` carrying task and purchase IDs | Fields seen in the spec's examples |
| Retrieve | `GET /payments/{payment_id}` | Listed in the API reference |
| Refund | `POST /refunds` | Listed in the API reference |
| Status mapping | `succeeded` → Succeeded; `failed` → Failed; `processing`, `requires_*` → Processing or ActionRequired; any other string → **Unknown** | The docs describe `requires_payment_method`, `requires_confirmation`, `requires_customer_action`, `processing`, `succeeded`, `failed` |
| Card data | Sandbox test card read from server config; the LLM and the dashboard never see it | Sandbox only |

### 9.3 Webhooks

1. Verify the signature in constant time. I recall Hyperswitch signing outgoing webhooks with an HMAC (header `x-webhook-signature-512`), but the pages I could open did not confirm it; read the webhook schema in the API reference and implement exactly that.
2. **A valid signature is only a trigger.** Extract the payment ID, call `retrieve`, and act on what the provider returns, not on the webhook body.
3. Deduplicate by provider event ID with a unique index; a replay is a no-op.
4. Terminal states are absorbing. A late `failed` after `captured` is not applied; it raises an incident.

### 9.4 Retries and circuit breaker

- Retry only `Timeout` and `ProviderDown`, always with the same deterministic payment ID, retrieving first. Up to 3 attempts, exponential backoff with jitter.
- Never retry `Declined`, `InsufficientFunds` or `InvalidRequest`; release the reservation.
- Per-provider breaker: opens after 5 consecutive failures, half-opens after a cool-down with one probe. While open, the router skips that provider and the decision trace says so. These thresholds are starting values to tune, not measured facts.

### 9.5 Reconciliation (the Hyperswitch echo)

A background task wakes every 30 seconds (5 seconds in demo mode) and does two passes.

- **Pass A, stuck purchases:** every purchase in `RESERVED` or `PENDING_UNKNOWN` older than a threshold is retrieved by its deterministic ID and moved to its true state. Provider returns not-found after a successful retrieve call: safe to release. Retrieve itself fails: keep holding.
- **Pass B, two-way compare:** for recent `CAPTURED` purchases, compare provider status, amount and currency with the ledger.

| Drift found | Automatic action |
| --- | --- |
| Provider succeeded, ledger pending | Capture, then issue grant |
| Provider failed, ledger captured | Revoke grant, open incident |
| Amount or currency differs | Freeze grant, open incident, no auto-heal |
| Provider has no record | Open incident, keep reservation held |

The drift counts feed benchmark claim 7 and a dashboard widget.

## 10. Audit ledger and fail-closed matrix

**The ledger is append-only and hash-chained per task, and every unknown in the system resolves to "hold the money, withhold the content, ask a human".** Both are testable, which is why they are the centre of the demo.

### 10.1 Ledger design

- Table `ledger_events(task_id, seq, type, payload jsonb, prev_hash, hash, created_at)` with `UNIQUE (task_id, seq)`. A trigger rejects `UPDATE` and `DELETE`.
- `hash = SHA-256(prev_hash || canonical_json(event))`. A verify endpoint recomputes the chain; the dashboard shows an "intact" badge. Tamper demo: disable the trigger in `psql`, edit one amount, refresh, and the badge turns red at the exact sequence number.
- Per-task chains, not one global chain, so concurrent tasks never contend on a shared tip.
- Event types: `task_created`, `offer_seen`, `policy_decided`, `approval_requested`, `approval_granted`, `reserved`, `provider_called`, `provider_result`, `captured`, `released`, `grant_issued`, `refund_recorded`, `drift_detected`, `incident_opened`.
- Each event carries the policy version, the rule trace, the idempotency key, the provider payment ID and amounts. Bearer tokens are stored only as hashes.

This fits section 5.4 of your working document and adds tamper-evidence, which is cheap and memorable.

### 10.2 Fail-closed matrix

Every row is an automated test. The test name column is what you put in the README.

| # | Fault or unknown | System behaviour | Money | Content granted? | Test |
| --- | --- | --- | --- | --- | --- |
| 1 | Policy JSON fails validation | Reject activation, keep previous version | Untouched | No | `policy_invalid_rejected` |
| 2 | Price, tax or fee unknown | RequireApproval | Untouched | No | `unknown_price_needs_approval` |
| 3 | Offer price changed after quote | Re-evaluate; if above reserved amount, release and deny | Released | No | `price_drift_denied` |
| 4 | Database down at reservation | 503, provider never called | Untouched | No | `db_down_no_provider_call` |
| 5 | Provider timeout | `PENDING_UNKNOWN`, reservation held, retrieve later | Held | No | `psp_timeout_holds` |
| 6 | Provider 5xx | Retry same ID with backoff, then hold | Held | No | `psp_5xx_retry_same_id` |
| 7 | Unmapped provider status string | Treated as Unknown, hold | Held | No | `unknown_status_holds` |
| 8 | Forged webhook signature | 401, logged, no state change | Untouched | No | `forged_webhook_rejected` |
| 9 | Duplicate webhook | No-op | Unchanged | Unchanged | `duplicate_webhook_noop` |
| 10 | Webhook says success, retrieve says failed | Trust retrieve, open incident | Released | No | `webhook_retrieve_mismatch` |
| 11 | Process killed after reservation, before provider call | Recovery finds `RESERVED`, retrieves by ID, retries or releases | Held then settled | Only after verify | `crash_after_reserve` |
| 12 | Process killed after provider success, before settle | Reconciler retrieves and captures | Held then captured | Only after verify | `crash_after_provider_ok` |
| 13 | Same idempotency key, same body | Replay stored response | One charge | Same grant | `idempotent_replay` |
| 14 | Same key, different body | 422 | Untouched | No | `key_reuse_conflict` |
| 15 | LLM output malformed or injected | Reject, scripted planner takes over | Untouched | No | `llm_garbage_rejected` |
| 16 | Fetched content says "raise the budget" | Ignored; agent credential has no policy-write scope | Untouched | No | `prompt_injection_no_effect` |
| 17 | Grant used on a different resource or after expiry | 403 | n/a | No | `grant_scope_enforced` |

Tests 11 and 12 use failpoints (an environment variable such as `FAULT_POINT=after_reserve` makes the process exit at that step), then restart the service and assert the invariants below. This is the strongest single piece of evidence in the project.

### 10.3 Invariants checked after every test and every property run

1. For every task: captured plus reserved never exceeds the cap.
2. Every grant row points to a purchase in `CAPTURED` that has a verified provider payment ID.
3. Every `CAPTURED` purchase has exactly one provider payment ID and at most one successful charge at the provider.
4. Every ledger chain verifies.
5. Sum of ledger `captured` amounts equals the budget row's captured amount.

## 11. Agent layer, guardrails, routing and anomaly detection

**The agent is a thin loop around an untrusted planner; prices come from provider offers, never from the model, and the model holds no credential that can move money.** The Data Science lines of the JD (routing, self-healing, anomaly detection) are answered by two small pure functions on top of this.

### 11.1 Agent loop

1. Planner (LLM or scripted) returns one JSON action: `RequestResource { provider, resource_id, purpose }`, `AskUser { question }` or `Finish { answer }`. Anything that fails schema validation is discarded.
2. Orchestrator calls the provider gateway. Free resource: use it. `402` with an offer: normalise it into the internal `Offer` type.
3. Offer is checked for authenticity and freshness (below), then `decide()` runs, then the purchase flow in section 8.
4. Result goes back to the planner as structured, size-limited data. Loop ends on `Finish`, a step limit (8), a purchase limit, or a wall-clock timeout.

### 11.2 Guardrails

| Threat | Control |
| --- | --- |
| Model invents or alters a price | Purchase API takes only an `offer_id`; the amount is read from the stored offer, never from the request body |
| Offer forged or tampered with | Provider simulators sign offers with an HMAC; unsigned or expired offers are untrusted and cannot be Allow |
| Prompt injection inside fetched content | Content is passed as data in a delimited field, never merged into instructions; the agent credential has no policy-write or budget-write scope; policy changes need the owner credential |
| Runaway loops | Step, purchase and time limits; a `stop` endpoint the owner can hit at any time |
| Secret leakage | Provider keys and the sandbox card live only in server environment variables; the planner prompt contains neither |

The offer JSON extends the one in your working document with `provider_id`, `expires_at`, `tax_minor`, `fee_minor`, `price_known` (boolean) and `offer_sig`.

### 11.3 Provider simulators (one binary, six behaviours)

| Provider | Behaviour | Demonstrates |
| --- | --- | --- |
| A | Free metadata | Free path, no transaction |
| B | ₹10 resource | Allowed purchase |
| C | ₹15 resource | Second purchase, cap tracking |
| D | ₹40 resource | Over-budget denial |
| E | Flaky: random 5xx and slow responses | Retries, circuit breaker, unknown-state handling |
| F | Price rises between quote and pay | Price-drift denial |

The resource gateway is the provider-side enforcement point: it serves content only for a grant minted in the ledger transaction, checks scope and expiry, and otherwise answers `402`.

### 11.4 Smart routing (policy first, then rank)

When several providers offer a comparable resource, the router ranks only the offers that policy allowed. It can never pick something policy denied.

```
score = w_price · norm(price) + w_fail · (1 − success_ewma) + w_lat · norm(p95_latency) + w_age · norm(data_age)
```

`success_ewma` is an exponentially weighted success rate per provider updated after every purchase outcome. Weights are configurable per tenant. The winning and losing scores are written to the ledger so the choice is explainable. This is a deliberate small-scale echo of Hyperswitch's success-rate routing, and the README should say so.

### 11.5 Spend anomaly detection

A pure function computes a robust z-score of each new purchase against the tenant's recent history (median and median absolute deviation) and a velocity count (purchases in a short window). The result is exposed to the DSL as `anomaly_score`, so a user rule such as "anomaly\_score above 3 requires approval" is one line. Test it on synthetic series with injected spikes and report precision and recall from your own generator, labelled as synthetic.

## 12. Data model, API surface and repo layout

**Three tables carry the guarantees: `budgets` (cap enforced by a CHECK), `purchases` (a closed set of states, one provider payment ID) and `grants` (a composite foreign key that only a CAPTURED purchase can satisfy).**

### 12.1 Core DDL

```sql
CREATE TYPE purchase_status AS ENUM
  ('AWAITING_APPROVAL','RESERVED','PENDING_UNKNOWN','CAPTURED','RELEASED','DENIED');

CREATE TABLE budgets (
  task_id        uuid PRIMARY KEY REFERENCES tasks(id),
  currency       text   NOT NULL,
  cap_minor      bigint NOT NULL CHECK (cap_minor >= 0),
  reserved_minor bigint NOT NULL DEFAULT 0 CHECK (reserved_minor >= 0),
  captured_minor bigint NOT NULL DEFAULT 0 CHECK (captured_minor >= 0),
  version        bigint NOT NULL DEFAULT 0,
  CHECK (reserved_minor + captured_minor <= cap_minor)
);

CREATE TABLE purchases (
  id                  uuid PRIMARY KEY,
  task_id             uuid NOT NULL REFERENCES tasks(id),
  offer_id            uuid NOT NULL REFERENCES offers(id),
  idempotency_key     text NOT NULL,
  amount_minor        bigint NOT NULL CHECK (amount_minor > 0),
  status              purchase_status NOT NULL,
  provider_payment_id text UNIQUE,
  policy_version      int  NOT NULL,
  created_at          timestamptz NOT NULL DEFAULT now(),
  UNIQUE (id, status),
  UNIQUE (task_id, idempotency_key)
);

CREATE TABLE grants (
  id              uuid PRIMARY KEY,
  purchase_id     uuid NOT NULL UNIQUE,
  purchase_status purchase_status NOT NULL DEFAULT 'CAPTURED'
                  CHECK (purchase_status = 'CAPTURED'),
  token_hash      bytea NOT NULL,
  scope           text  NOT NULL,
  expires_at      timestamptz NOT NULL,
  revoked_at      timestamptz,
  FOREIGN KEY (purchase_id, purchase_status)
    REFERENCES purchases (id, status) ON UPDATE RESTRICT
);
```

`CAPTURED` is absorbing: a purchase never leaves it. A later provider disagreement sets `revoked_at` on the grant and opens an incident rather than rewriting history. Correction to section 8.5: the "grant needs a verified payment" rule is enforced by this composite key, not a plain foreign key.

Remaining tables, one line each:

| Table | Key columns | Purpose |
| --- | --- | --- |
| `tenants`, `api_keys` | hashed key, scope (`owner`, `agent`, `provider_webhook`) | Isolation and least privilege |
| `tasks` | goal, status, policy\_version, autonomy | Groups one user goal |
| `policies` | version, json, state (`draft`, `shadow`, `active`, `retired`) | Immutable once active |
| `offers` | provider\_id, resource\_id, amount, tax, fee, price\_known, expires\_at, sig | What was quoted |
| `idempotency_keys` | tenant, key, request\_hash, status, response | Replay and conflict detection |
| `ledger_events` | task\_id, seq, type, payload, prev\_hash, hash | Tamper-evident audit |
| `webhook_events` | provider\_event\_id UNIQUE, received\_at | Deduplication |
| `approvals` | purchase\_id, requested\_at, decided\_by, decision | Human-in-the-loop |
| `provider_stats` | provider\_id, success\_ewma, p95\_latency | Router inputs |
| `incidents` | kind, purchase\_id, opened\_at, resolved\_at | Drift and mismatch cases |

### 12.2 API surface

| Method and path | Credential | Purpose |
| --- | --- | --- |
| `POST /v1/tasks` | owner | Create task with budget and policy version |
| `POST /v1/tasks/{id}/stop` | owner | Kill switch |
| `POST /v1/policies`, `/{v}/dry-run`, `/{v}/shadow`, `/{v}/activate` | owner | Self-service policy lifecycle |
| `POST /v1/tasks/{id}/purchases` (header `Idempotency-Key`, body `{offer_id}`) | agent | Run decision, reservation, payment |
| `POST /v1/approvals/{id}/approve` and `/deny` | owner | Human approval |
| `GET /v1/resources/{provider}/{resource}` | agent | Provider gateway: `402` with offer, or content with a valid grant |
| `POST /v1/webhooks/hyperswitch` | provider\_webhook | Signed trigger for a retrieve |
| `GET /v1/tasks/{id}/ledger` and `/ledger/verify` | owner | Audit trail and chain check |
| `GET /v1/tasks/{id}/stream` | owner | Server-sent events for the live dashboard |
| `POST /v1/admin/reconcile` | owner | Run reconciliation now (demo button) |
| `GET /metrics` | internal | Prometheus counters and histograms |

### 12.3 Repository layout

```
accesspilot/
├─ AGENTS.md                 # rules for Antigravity (section 14)
├─ docker-compose.yml        # postgres, optional hyperswitch
├─ crates/
│  ├─ ap-core/               # PURE: money, ids, Policy AST, decide(), state machines, router, anomaly
│  ├─ ap-store/              # sqlx: budgets, purchases, idempotency, ledger, grants, migrations/
│  ├─ ap-psp/                # PaymentProvider trait, hyperswitch.rs, simulated.rs (fault injection)
│  ├─ ap-agent/              # planner clients (groq, gemini, scripted), action schema, guardrails
│  ├─ ap-gateway/            # axum app: API, webhook receiver, provider simulators, SSE, /metrics
│  └─ ap-harness/            # load, chaos, property and reconciliation tests; prints the results table
├─ dashboard/                # one static page (HTML + htmx or Alpine), served by the gateway
├─ scripts/                  # run_bench.sh, seed.sh, demo.sh
└─ docs/                     # ARCHITECTURE.md, BENCHMARKS.md, DEMO.md
```

Dependency rule: `ap-core` depends on nothing but `serde`, `sha2` and a decimal-free money type; everything else depends on it. A reviewer can read `ap-core` alone and see the whole business logic.

## 13. Tests and benchmarks that produce the output numbers

**One command, `make bench`, runs the harness and prints a results table; that table, pasted at the top of the README, is your "impressive numbers".** Every number is measured on your machine and labelled with its hardware, because a reviewer who reruns it and gets the same order of magnitude trusts the rest of the repository.

### 13.1 The ten harness tests

| ID | Test | Setup | Pass criterion (target) |
| --- | --- | --- | --- |
| T1 | Cap under concurrency | One task, ₹50 cap, 5,000 concurrent purchase attempts of random ₹1–₹20 amounts, simulated PSP | Captured plus reserved never above cap; 0 violations; report accepted vs denied counts |
| T2 | Idempotency storm | 1,000 concurrent requests, one key | Simulated PSP charge counter equals 1; 999 replays or 409s |
| T3 | Fail-closed matrix | The 17 rows in section 10.2 | 17 of 17 pass |
| T4 | Crash recovery | Failpoint kill at each step across 200 random purchases each, restart, reconcile | 0 orphaned reservations, 0 invariant failures |
| T5 | Model-based and property tests | proptest: 100,000 policy cases; stateful runs of random event sequences (offer, approve, timeout, duplicate webhook, crash) compared against a pure reference model in `ap-core` | DB-backed implementation agrees with the model; 0 failures |
| T6 | Latency and throughput | Criterion for `decide()` with a 20-rule policy; load run of `POST /purchases` with 0 ms simulated PSP, 5,000 attempts spread over 500 tasks, then all on one task | Decision p99 under 200 µs; request p99 under 25 ms; 1,000+ requests/s spread; report the single-task figure separately |
| T7 | Reconciliation | Inject 50 drifts (10 each of 5 kinds) | 50 of 50 detected; auto-heal applied only where the table in 9.5 allows |
| T8 | Anomaly detector | Synthetic series with injected spikes | Report precision and recall from your generator, labelled synthetic |
| T9 | Ledger tamper | 100 random single-field mutations | 100 detected, each at the correct sequence number |
| T10 | Hyperswitch sandbox smoke | 10 payments through the Dummy Connector: successes and declines, each retrieved and matched | 10 of 10 reconcile; report sandbox latency p50 and p95, noting it is network-bound |

The targets are plausible for Rust plus a local Postgres, but I have not run them; they are what you aim to demonstrate, not what you may claim in advance.

### 13.2 Output format the harness should print

Placeholders only: the harness fills them, you never type them.

```
AccessPilot harness  |  <date>  |  <cpu> / <ram>  |  Postgres 16 (Docker)  |  rustc <ver>  |  seed <n>
T1  cap_under_concurrency   attempts=<n> accepted=<n> denied=<n> captured=<minor> cap=<minor> violations=<n>   PASS|FAIL
T2  idempotency_storm       requests=<n> provider_charges=<n> replays=<n>                                     PASS|FAIL
T3  fail_closed_matrix      passed=<n>/17                                                                     PASS|FAIL
T4  crash_recovery          failpoints=<n> runs=<n> orphans=<n> invariant_failures=<n>                         PASS|FAIL
T5  model_and_properties    cases=<n> disagreements=<n>                                                       PASS|FAIL
T6  latency                 decide_p99=<us> req_p50=<ms> req_p99=<ms> rps_spread=<n> rps_single_task=<n>       INFO
T7  reconciliation          drifts=<n> detected=<n> healed=<n> incidents=<n>                                   PASS|FAIL
T9  ledger_tamper           mutations=<n> detected=<n>                                                        PASS|FAIL
T10 hyperswitch_sandbox     payments=<n> matched=<n> p50=<ms> p95=<ms>                                         INFO
```

### 13.3 Method notes that keep the numbers credible

- Warm up, run three times, report the median; print the hardware and the random seed.
- State clearly that T1 to T9 use the simulated PSP so they measure AccessPilot, not a network, and that T10 measures the sandbox.
- Keep raw outputs in `docs/BENCHMARKS.md` and run the unit and property tests (not the long load runs) in GitHub Actions, which is free for public repositories.
- If a target is missed, publish the real figure and one sentence on what bounds it (for example, the single-row lock in a hot task). Reviewers read candour as competence.

## 14. Build timeline and Antigravity prompt pack

**A six-hour plan builds the pure core first and the pretty parts last, so if time runs out you still submit the strongest 70% of the project.** The timeline below assumes about six hours; scale the phases down evenly if you have less, and apply the cut order underneath.

### 14.1 Cut order if time runs short

Drop from the top of this list first. Never drop the last four.

1. Hosting a live URL (the screen recording is enough).
2. The rule-builder form (keep the JSON editor and the dry-run button).
3. Smart router and anomaly detector (the DSL field can return 0).
4. The live LLM planner (keep the scripted planner).
5. Live Hyperswitch smoke test T10 (keep the adapter and its mock-server tests).
6. **Never drop:** pure core with property tests, atomic reservation, idempotency, ledger with fail-closed tests.

### 14.2 Rules file for Antigravity (`AGENTS.md`)

Put this at the repo root before the first prompt, so every agent run starts with the same constraints.

```markdown
# AccessPilot rules
- Money is i64 minor units + a Currency enum. Never use floats for money.
- crates/ap-core: no I/O, no async, no unwrap/expect/panic, no `as` casts on money. Pure functions and enums only.
- Every lifecycle (purchase, reservation, grant, policy) is an enum with a total transition function.
- Never hold a database transaction open across a network call.
- A state change and its ledger event are written in the same transaction.
- Unknown input, unknown status, unknown price => hold money, withhold content, ask a human. Never default to Allow.
- Write the failing test first. Before finishing any task run: cargo fmt && cargo clippy -- -D warnings && cargo test
- Do not guess Hyperswitch field names or headers. Check https://api-reference.hyperswitch.io; if unsure, add a TODO and a failing test.
- Secrets come from environment variables. Never print or commit them.
- Do not mark a task done until its acceptance test passes and you have shown the output.
```

### 14.3 Prompt pack (paste one at a time; each ends with its acceptance test)

**P0 · Scaffold (gemini-3.8-flash).** "Create a Cargo workspace with crates ap-core, ap-store, ap-psp, ap-agent, ap-gateway, ap-harness, a docker-compose.yml with Postgres 16, a Makefile with targets `up`, `test`, `bench`, `demo`, and a GitHub Actions workflow running fmt, clippy and unit tests. No business logic yet. Acceptance: `make up && cargo test` succeeds."

**P1 · Pure core (claude-opus-4.6).** "In ap-core implement: `Money` and `Currency`; ID newtypes; the Policy AST and JSON (serde) format from section 7 of ARCHITECTURE.md; `decide(policy, ctx) -> Decision` with the hard-invariant layer, most-restrictive-wins rules, default Deny, and a rule trace; purchase, reservation and grant state enums with total transition functions; a pure reference model of the whole purchase lifecycle. Add proptest properties: permutation invariance, price monotonicity, budget safety, unknown-is-never-Allow, JSON round trip. Acceptance: `cargo test -p ap-core` green and clippy clean with unwrap/panic denied."

**P2 · Store (claude-opus-4.6).** "In ap-store write sqlx migrations from section 12.1 and these functions: `claim_idempotency`, `reserve` (the single atomic UPDATE from section 8.2), `settle_captured` (one transaction: purchase to CAPTURED, reservation to captured, grant insert, ledger events), `release`, `mark_pending_unknown`, and `append_ledger_event` with the per-task hash chain and an UPDATE/DELETE-rejecting trigger. Acceptance tests: T1 (5,000 concurrent attempts, 0 violations) and T2 (1,000 concurrent same-key requests, one charge)."

**P3 · Provider layer (claude-sonnet-4.6).** "In ap-psp define the `PaymentProvider` trait from section 9.1. Implement `SimulatedPsp` with configurable latency, error rate, timeout, duplicate and delayed webhooks, a charge counter and deterministic IDs. Implement `HyperswitchPsp` against https://sandbox.hyperswitch.io reading the key from `HYPERSWITCH_API_KEY`; confirm auth header and field names from the OpenAPI spec; map unknown statuses to Unknown. Add a wiremock-based test suite for the adapter. Acceptance: adapter tests green; status-mapping table test covers every status string in the docs."

**P4 · Purchase flow and reconciler (claude-opus-4.6).** "In ap-gateway implement the purchase flow of section 8.3 exactly (claim key, tx1, provider call with deterministic ID, tx2 by outcome, store response), a `FAULT_POINT` env var that exits the process after named steps, the webhook endpoint (verify, dedupe, retrieve, apply), and the reconciler of section 9.5. Acceptance: T3 rows 4 to 14 and T4 pass."

**P5 · Providers and agent (claude-sonnet-4.6).** "Implement the six provider simulators of section 11.3 with signed offers, plus the resource gateway that serves content only for a valid grant. In ap-agent implement the action schema, the scripted planner, and Groq and Gemini planners behind one trait with strict schema validation and automatic fallback to the scripted planner. Acceptance: the end-to-end demo script runs with the scripted planner and with a live LLM."

**P6 · Dashboard (gemini-3.8-flash).** "Build one static page served by ap-gateway: task form, policy JSON editor with Dry-run and Shadow buttons, a live ledger timeline over server-sent events, a budget meter that shows the cap as a limit (not a balance) with reserved, captured and remaining, a ledger-chain badge, an incident list, and buttons for the idempotency storm, the crash demo and run-reconcile. No build step."

**P7 · Harness and docs (claude-sonnet-4.6, then gemini-3.8-flash for prose).** "Implement ap-harness tests T1 to T10 from section 13 and print the exact table format in 13.2 with real measurements, hardware and seed. Then generate README.md (architecture diagram first, results table second, quick start third) and docs/BENCHMARKS.md from the real output. Acceptance: `make bench` prints the table with no placeholders."

## 15. Demo script, submission checklist, interview defence, and what is unverified

**The demo is three minutes and every beat ends on something a reviewer can verify: a ledger row, a counter, a red badge, or a printed table.** It reuses your seven scenes and adds the three that only this architecture can show.

### 15.1 Three-minute demo

| Time | Beat | What the screen shows |
| --- | --- | --- |
| 0:00 | Set the rules | Policy JSON beside the form; budget meter says "₹50 limit (not a balance)" |
| 0:20 | Free resource | Provider A used; no transaction row |
| 0:35 | Two purchases | B ₹10 and C ₹15 allowed; ledger shows reserved, provider result, captured, grant for each |
| 1:00 | Blocked purchase | D ₹40 denied: "expected total ₹40.00 exceeds remaining ₹25.00"; no provider call logged |
| 1:20 | Idempotency storm | Button fires 1,000 identical requests; counter shows 1 charge |
| 1:45 | Crash | Process killed mid-purchase; restart; reconciler settles it; budget never exceeded |
| 2:10 | Fail-closed | Flaky provider E times out; purchase sits in PENDING\_UNKNOWN, content stays locked until verified |
| 2:30 | Tamper | Edit a ledger row in `psql`; the badge turns red at the exact sequence number |
| 2:45 | Numbers | `make bench` table on screen; one-line close |

Close line: "Every rupee and every decision is explainable, and the system fails safe when it does not know."

### 15.2 Submission checklist

- [ ] Public repository with a README whose first screen is the architecture diagram and the printed results table
- [ ] `make up && make demo` works from a clean clone
- [ ] `.env.example` present; no keys, cards or tokens in git history
- [ ] `docs/ARCHITECTURE.md` (trimmed from this document) and `docs/BENCHMARKS.md` (raw harness output with hardware)
- [ ] GitHub Actions badge: fmt, clippy, unit and property tests
- [ ] Demo recording, 3 minutes, with captions for the key beats
- [ ] README section "Design decisions" covering: Postgres reservation over Redis, why Rust in functional style, order-independent rules, hold on unknown
- [ ] README section "Not in scope": real money, PCI scope, wallets and escrow, multi-region
- [ ] Repo description and pinned topics: payments, rust, idempotency, hyperswitch

### 15.3 How to word it in the application

Use measured numbers only; fill the braces from your own table.

- Built AccessPilot, a Rust payments control plane for AI-agent spend on Hyperswitch: atomic budget reservation and idempotent retries held {N} concurrent attempts to {violations} cap violations and {charges} duplicate charges.
- Expressed spending rules as a typed policy DSL evaluated by a pure function ({p99} µs p99), verified by {cases} property cases and a model-based test against a reference state machine.
- Designed fail-closed behaviour for {rows} fault scenarios including crash-recovery at {failpoints} failpoints, with a hash-chained audit ledger and ledger-vs-provider reconciliation.

### 15.4 Questions to be ready for

| Question | Short answer |
| --- | --- |
| Why reserve instead of checking the balance first? | A check-then-pay has a race window; the reservation is one atomic statement, and a database CHECK guards it even if the code is wrong |
| What if the provider times out? | The outcome is unknown, so the money stays reserved and the content locked; a retrieve by the deterministic payment ID decides |
| How do you know there is no double charge? | Idempotency key plus deterministic provider ID plus retrieve-before-create; T2 and the crash tests demonstrate it |
| Why Rust and not Haskell? | Time, plus Hyperswitch is Rust; the design uses the same ideas (algebraic types, pure core, total functions) and ports directly |
| Why Postgres and not Redis for the hot path? | Money correctness first; the Redis-plus-Lua reservation with write-behind is the next experiment, measured against T1 and T2 |
| What breaks at 5,000 TPS? | The hot budget row and a single Postgres writer; shard by task, batch ledger writes, move webhook handling to a queue |
| Why order-independent rules? | Non-programmers edit rules in a form; the result must not depend on row order, and it is a testable property |
| What is not covered? | PCI scope (cards stay in the provider's sandbox), real money movement, multi-region, provider contracts |

### 15.5 Unverified items: settle these in the first 20 minutes

I could not confirm these from pages I could open; each has a fallback so none blocks the build.

| Item | Why it matters | Fallback |
| --- | --- | --- |
| Hyperswitch auth header name and exact `POST /payments` fields | Adapter requests | Read the OpenAPI spec; mock-server tests until confirmed |
| Whether Hyperswitch rejects a repeated `payment_id` | Idempotency design | Design already retrieves before creating, so it does not rely on this |
| Webhook signature header and algorithm | Webhook verification | Read the webhook schema in the API reference; retrieve-on-trigger limits the risk |
| Which test cards drive Dummy Connector successes and failures | Demo script | Use the simulated PSP for failures |
| Cloud sandbox signup friction | Time | Self-hosted Docker or simulated PSP |
| Antigravity weekly quota and current free model list | Build pace | Spend Opus 4.6 on P1, P2 and P4 first; fall back to gpt-oss-120b |
| Free LLM limits and tool-calling quality | Live planner | Scripted planner |
| Free hosted Postgres limits | Optional live URL | Skip hosting |
| Which interview rounds apply to this role | Preparation | Not covered here; prepare data structures separately |

### Sources

- [Juspay careers, DEV-BE02](https://juspay.io/careers/DEV-BE02) and [SDE Functional Programming](https://juspay.in/sde-functional-programming), opened 3 Nov 2026
- [Job-board listing of the backend intern role](https://jobs.weekday.works/juspay-software-development-engineer-backend-intern)
- [Juspay engineering principles article](https://dev.to/hyperswitchio/scaling-to-125-million-transactions-per-day-juspays-engineering-principles-2bj1)
- [Hyperswitch repository](https://github.com/juspay/hyperswitch), [API reference](https://api-reference.hyperswitch.io/introduction), [test credentials](https://docs.hyperswitch.io/integrations/connectors-integrations/payment-processor-capabilities/payment-methods-setup/test-credentials), [connector testing guide](https://github.com/juspay/hyperswitch-docs/blob/main/hyperswitch-cloud/connectors/test-a-payment-with-connector.md)
- [Antigravity free-tier tracker](https://mvalentsev.github.io/awesome-free-ai-coding/providers/antigravity/) (third party, probed 2 Nov 2026)
- [OpenRouter free LLM API comparison](https://openrouter.ai/blog/tutorials/free-llm-apis-compared/) (vendor source)
- [Free hosting tiers roundup](https://flaviocopes.com/hosting-free-tiers.md) (checked Sept 2026)
- Your working document, AccessPilot v1.0 (30 Oct 2026)
