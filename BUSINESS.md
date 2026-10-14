# BUSINESS.md: Ex8:Margin — Business Requirements & Functionality

> Audience: product owners, business analysts, and anyone evaluating what
> Ex8:Margin **does for the business**. For architecture read `README.md`;
> for wire-level detail read `DESIGN.md`.

## 1. Business Purpose

- Protect the venue and its participants from derivatives credit risk: keep
  every leveraged position (futures, perpetuals, options) covered by margin
  that is measured, marked, and enforced continuously — before losses can
  cascade.
- Guarantee that under-collateralized accounts are closed early and
  precisely: the maintenance-margin wall triggers a quantified shortfall, a
  defined close plan, and an auditable liquidation record.
- Keep the perpetuals market pinned to the index: a clamped funding rate
  transfers value between longs and shorts so the mark never drifts far from
  the underlying.
- Price options with defensible, replayable math: closed-form Greeks for
  European contracts, a tree model that captures the American early-exercise
  premium, and portfolio-level risk limits expressed in Greeks, not notional.
- Settle expiring contracts exactly once, at a transparent settlement price
  formed from an observation window, with either cash variation or physical
  delivery obligations.

## 2. Business Scope

### What the service provides

| Capability | Description |
|---|---|
| Position ledger | Net signed position, average entry, realized P&L per (env, account, contract), event-sourced from the trade stream and reconciled against clearing facts |
| Mark-to-market | Periodic (Sentin8:Cron) and index-tick-driven revaluation of open positions with a durable last-mark trail |
| Margin calculation | Initial and maintenance margin as configurable basis-point requirements; per-contract margin model declared (isolated / cross / span / portfolio) |
| Margin calls | Margin-call events with requirement, shortfall, and the mark the decision ran against |
| Liquidation | Close-plan engine: close quantity, close side, shortfall — closed through the market (Ex8:ME), with a durable pending → submitted → filled/failed lifecycle |
| Perpetual funding | Periodic funding between longs and shorts from the clamped premium index, with an applied-rate history for every posting |
| Options Greeks | Closed-form (Black–Scholes–Merton) delta/gamma/vega/theta/rho for European options, cached per contract |
| American pricing | Cox-Ross-Rubinstein tree pricing/Greeks capturing the early-exercise premium |
| Portfolio netting | Account-level Greeks: underlying legs (±qty) + option legs (qty × multiplier × per-unit Greek); delta-equivalent notional, gamma, vega risk limits |
| Futures/option expiry | Settlement-price observation windows, cash P&L settlement, physical delivery obligations, idempotent settlement state machine (open → expiring → settled) |
| Query surfaces | RLS-scoped positions, margin-account, and liquidation views (+ RFW admin aliases) for dashboards and apps |
| Admin surfaces | Shield policy administration (shared platform mount) and declared RFW admin pages (positions / accounts / liquidations) |

### What the service does NOT own (business boundary)

| Concern | Owning service |
|---|---|
| Pre-trade authorization and opening-trade risk gating (including the options-layer checks that consume Greeks values) | Ex8:Risk |
| Matching and the execution of the liquidation close order | Ex8:ME |
| Trade persistence, clearing, busts/amendments, exercise/assignment workflow | Ex8:Clearing |
| Index composition, publication, and staleness health | Ex8:VFeeds |
| Contract reference data and corporate-action position adjustments (fed in as events) | Ex8:RefData |
| External settlement rails for cash variation and physical delivery | Ex8:Settlement |
| Fiat margin rails and custody movements (postings behind funding/deposits) | Bank8:Ledger / Ex8:Settlement custody |
| Order entry, client protocols, session management | Ex8:Gateway / Ex8:OMS |
| Market surveillance of the flows margin exposes | Ex8:Surveillance |
| Scheduling of margin/MTM/funding/expiry cycles | Sentin8:Cron |
| Identity, tokens, authorization decision engine | Sentin8:Auth / Shield (declared sets) |
| Event durability & fan-out infrastructure | Sentin8:Bus |
| Audit archival | Sentin8:Sys |

## 3. Business Capabilities

### Margin & position accounting

| Capability | Business rule |
|---|---|
| Position fidelity | A contract position carries one net signed quantity, one volume-weighted average entry, and one realized-P&L accumulator; additions re-weight the average in absolute-quantity space so the sign never cancels |
| Realization discipline | Reducing a position realizes P&L against the average entry and keeps the average; crossing zero realizes the entire old position once, then rebuilds from the new price; closing flat resets the entry and preserves realized P&L |
| Mark-to-market variation | Effective equity equals posted equity plus unrealized P&L at the mark — cash-settled variation is part of the margin picture, not an afterthought |
| Initial vs maintenance | The initial requirement (default 100% of notional) always exceeds the maintenance requirement (default 50%); entering a position demands the higher bar, staying in it is governed by the lower one |
| Requirement transparency | Every margin decision carries the requirement, the mark it was computed from, and (on breach) the exact shortfall |

### Mark-to-market & funding

| Capability | Business rule |
|---|---|
| MTM idempotence | Re-running a mark at the same price yields the same variation — marks are recomputed, never appended blindly |
| Feed-stall posture | While the index feed is unavailable, margin holds the last known mark; liquidation decisions remain auditable against that mark |
| Funding sign | Longs pay when the premium index is positive; shorts receive; the sign flips below zero — the direction always works to pull the mark toward the index |
| Funding clamp | Funding never exceeds the configured cap in either direction; the applied (clamped) rate is stored with every posting |
| Posting, not a trade | Funding is a value transfer between long and short accounts recorded as a posting — it never prints a trade or touches the book |

### Liquidation

| Capability | Business rule |
|---|---|
| Maintenance wall | When equity (including unrealized variation) falls below the maintenance requirement, the account is breached and the shortfall quantified |
| Defined close plan | The close order is exact: the absolute position size, on the side opposite the position (longs are closed by selling, shorts by buying) |
| Ordered lifecycle | Every liquidation is a durable record that progresses pending → submitted → filled/failed; failed closes retry and escalate to admin liquidation, audited |
| No flat liquidations | Flat accounts are never in a liquidation plan — margin events fire only where a real obligation exists |
| Margin-call surface | A margin-call event accompanies breaches (error code `exchange8_margin_call_open`, HTTP 409) so upstream clients can add collateral |

### Greeks & options risk

| Capability | Business rule |
|---|---|
| Closed-form Greeks | European options are priced and riskified with Black–Scholes–Merton (price, delta, gamma, vega, theta, rho) validated against textbook values and put–call parity |
| American pricing | Early-exercise-premium contracts price through the CRR tree with early exercise at every node; the tree converges to the closed form when exercise is worthless |
| Portfolio netting | Risk is netted per account: underlying positions contribute full delta; option legs contribute qty × multiplier × per-unit Greeks — a hedged book is measured as one position |
| Greeks-shaped limits | Options risk limits act on delta-equivalent notional (delta × underlying price), gamma exposure, and vega exposure — absolute values, so short books trip the same limits |
| Degenerate protection | Non-positive spot, strike, volatility, or time is rejected — no option is quoted or risk-checked from a degenerate input |
| Greeks cache | Latest Greeks per option are cached for dashboards and the options-layer checks |

### Expiry & settlement outcomes

| Capability | Business rule |
|---|---|
| Settlement price formation | The final settlement price is the arithmetic mean of the observation window — a single transparent number, reproducible by anyone who can see the ticks |
| Cash settlement | Expiring cash contracts book each account's P&L as quantity × (settlement − entry); longs and shorts mirror exactly |
| Physical delivery | Delivery-style contracts produce signed delivery obligations (long receives, short delivers), handed to external settlement |
| Settle-once | A contract settles at most once; a crashed settlement replays to the recorded result rather than booking twice |
| Window discipline | A contract with no settlement observations cannot be settled — it stays expiring until the window actually accrues |

## 4. Business Events (Outbound)

Emitted domain events (vocabulary `exchange8:margin:{resource}:{operation}`;
the concrete bus event strings/payload envelopes are the open writer surface —
delivery is idempotent via the local outbox → Sentin8:Bus):

| Event family | When it fires |
|---|---|
| Margin call | A maintenance check breaches and the shortfall is quantified |
| Liquidation raised | The close plan is created for a breached account (with its pending → submitted → filled/failed progress) |
| Funding applied | A funding cycle moves value between longs and shorts on a perpetual contract |
| Position updated | A fill, corporate-action adjustment, expiry, or liquidation changes the net position |
| Expiry settled | A contract transitions to settled (final price + cash P&L or delivery obligations) |

Inbound events consumed: trade executions and clearing facts (position
accounting + reconciliation), index updates (marks + premium index),
settlement-due triggers (expiry/final variations), corporate-action position
adjustments, Shield policy-set updates (hot reload).

## 5. Business Rules & Invariants

1. A margin requirement is always visible with the mark it was computed
   from: requirement = |position| × mark × basis-point fraction.
2. Effective equity equals posted equity plus unrealized variation at the
   current mark — no "hidden" P&L changes a margin decision.
3. Initial margin is always at least maintenance margin; entering is harder
   than staying.
4. A flat account can never be breached; liquidations only apply to real
   obligations.
5. The close plan always closes the entire remaining position, on the side
   opposite the position; the shortfall is carried with the plan.
6. Realized P&L is conserved: every fill path (add, reduce, flip, close)
   leaves the ledger balanced, and a closed position keeps its realized P&L
   with a zeroed entry.
7. Funding never exceeds its cap in either direction, and every applied rate
   is stored before the value moves — funding is auditable and reproducible.
8. A contract settles at most once, at the mean of its observation window;
   cash and physical outcomes are mutually exclusive per contract.
9. No tenant (environment) can ever see or influence another environment's
   margin ledger: row-level security is force-enabled on every margin table
   and context is set on the same connection that queries.
10. Authorization is fail-closed and decided only by the Shield engine's
    seeded scope mirror: no set/no match → deny, shadow mode never blocks,
    tenant forbids override platform permits, machine tokens follow the same
    policy as users.
11. Risk events persist durably in the same transaction as the state change
    that produced them, and are delivered at-least-once with an idempotency
    key — delayed or retried, never silently dropped.
12. Audit trails are delayed, never lost: every decision/change is journaled
    and pushed to the platform audit ingest with retries surviving outages.
13. Scheduling is single-sourced: margin, MTM, funding, and expiry cycles run
    on the platform scheduler — no replica-local timers double-fire cycles.
14. Degenerate market inputs are refused, not defaulted: an option without a
    positive price path (spot/strike/vol/time) is never priced or
    limit-checked.
15. The margin ledger reconciles to the trade stream: position accounting is
    event-sourced, so a dispute resolves by replaying the same events, not by
    trusting an unknown accumulator.

## 6. Target-User Roles & Use Cases

| Persona | Role | What they do with Ex8:Margin |
|---|---|---|
| Retail / institutional trader | Derivatives participant | Watches margin requirements and liquidation proximity on their positions; responds to margin calls (add collateral / reduce size) |
| Perpetuals trader / market maker | Funding participant | Reads funding history and clamping behavior; manages carry between long and short funding payments |
| Options trader | Hedged book owner | Views netted portfolio Greeks and delta-notional/gamma/vega headroom against limits |
| Sell-side broker | Collateral manager | Monitors client margin accounts and margin-call events; escalates funding before the close plan triggers |
| Exchange operations | Risk operator | Reviews liquidation events and shortfall quantification; escalates failed closes to admin liquidation; tunes maintenance/init basis points |
| Clearing & settlement | Consumer | Consumes expiry settlements (cash P&L, delivery obligations) and funding/liquidation outcomes for external rails |
| Surveillance & compliance | Consumer | Reviews margin-call/liquidation audit records (trigger condition, mark, shortfall) |
| Venue administrator | Shield admin | Manages margin policy sets (enforce/shadow), relations, and attributes via the shared admin API |
| Product / analytics | Dashboard consumer | Reads the RLS-scoped positions, accounts, and liquidation views through the app/BFF proxy |
| Platform engineering | Operator | Rolling upgrades with drain, outbox/audit delivery monitoring, and cycle scheduling health |

## 7. KPIs / Success Criteria

- **Decision latency**: liquidation decisions complete inside the design
  budget (millisecond-scale per account; sub-10 ms decision path) and MTM
  revalues each position at millisecond scale, at 100k-positions-per-env
  scale.
- **Zero missed breaches**: every account below its maintenance wall appears
  as a margin call + liquidation plan with an exact shortfall in the same
  cycle that detected it.
- **Deterministic risk math**: identical inputs produce identical
  requirement/plan results on replay; realized-P&L reconciliation across the
  trade stream shows no drift.
- **Funding integrity**: no funding cycle exceeds its clamp, none is applied
  twice, and every posting is reconstructible from the stored
  premium/cap/rate record.
- **Settlement exactness**: each expiring contract settles exactly once at
  the observation-window mean; crash-replay settlement tests show no double
  booking.
- **Isolation**: zero cross-environment information flow (RLS matrix green —
  no context → zero rows; cross-env → zero rows; contextless insert →
  rejected).
- **Authorization**: fail-closed policy behavior proven against the real
  seeded set (scope wiring tests), with shadow-mode rollback available
  without code.
- **Durability**: outbox and audit delivery survive Bus/Sys outages with
  zero-loss retries; DLQ replay is available for dead events.
- **Availability**: the query surface and cycles degrade gracefully — feed
  stalls hold last marks; platform outages delay (but never lose) events.

## 8. Dependencies & Reliance

- **Ex8:ME / Ex8:Clearing** — the trade stream that drives position
  accounting; clearing facts reconcile the ledger, and the close orders from
  liquidation plans execute in the market.
- **Ex8:VFeeds** — the index price for marks, the premium index behind
  funding, and the settlement-price formation input.
- **Ex8:RefData** — contract master context (underlying, corporate actions:
  position adjustments arrive as events at ex-dates).
- **Ex8:Risk** — the synchronous pre-trade gate; margin provides the
  asynchronous, stateful counterpart (and the Greeks values its options-layer
  checks consume).
- **Ex8:Settlement** — consumer of expiry outcomes (cash variation and
  physical delivery) and external-rail postings.
- **Sentin8:Cron** — schedules margin cycles, MTM runs, funding intervals,
  and expiry windows (no replica-local scheduler).
- **Sentin8:Auth** — token issuance/verification; the service's cert-bound
  M2M workloads (bus produce/consume, policy hot-reload, stat, audit push,
  TLS host resolution) are DCR-registered.
- **Sentin8:Policy (Shield)** — the only authorization gate; the seeded
  scope-mirror set is platform-declared and hot-reloadable.
- **Sentin8:Bus / Sentin8:MemBus** — durable event delivery (outbox relay)
  and realtime publication; both degrade without blocking risk operation.
- **Sentin8:Sys** — audit ingest (`POST /v1/audit`) and the TLS hostname
  resolution owner API (`GET /v1/resolve/hostname`).
- **Sentin8:DB (PostgreSQL 18, db `exchange8`, schema `margin`)** — the sole
  persistent state; never touched without a tenant context.
- **Platform shell (`sentin8::shell`)** — registration, capped DB pool,
  health, drain, allocator; and `sentin8-tls` for in-process post-quantum TLS.

## 9. Acceptance Criteria

1. Fill accounting: repeated fills across add/reduce/flip/close produce the
   documented average-entry and realized-P&L outcomes exactly (weighted
   average in absolute-quantity space; close to flat zeroes the entry).
2. A margin check reports the requirement, the effective equity, and — on
   breach — the exact shortfall at the current mark; flat accounts are
   excluded by construction.
3. Initial (default 100% bps) always exceeds maintenance (default 50% bps)
   for the same position and mark.
4. Liquidation plans close the full |position| on the correct side (long →
   sell, short → buy) and progress the durable pending → submitted →
   filled/failed lifecycle, with failed closes retrying and escalating.
5. Funding applies the clamped premium in both directions (longs pay /
   shorts receive when positive), never exceeds the cap, and stores the
   applied rate for every posting.
6. BSM Greeks match textbook values within tolerance; put–call parity and
   delta parity hold exactly; degenerate inputs are rejected; netting across
   underlying + option legs reproduces hand-computed portfolio values.
7. American options price through the CRR tree with early exercise (put ≥
   European, ≈ intrinsic deep ITM) and reuse the same Greeks shape as the
   closed form.
8. Futures expiry settles once at the mean of the observation window; cash
   contracts book mirrored P&L per account; physical contracts produce signed
   delivery obligations; idempotent re-settle returns the recorded result.
9. Every durable margin table is RLS-forced with provable env isolation
   (no context → zero rows; cross-env → zero rows; contextless insert →
   rejected) and the service role holds no bypass privileges.
10. Authorization rides the seeded Shield scope mirror only: fail-closed
    without sets, shadow never blocks, tenant forbids win, M2M parity holds,
    and every handler action is covered by the declared mirror.
11. Margin/liquidation/funding/expiry events persist in the same transaction
    as the state change, are delivered to the bus idempotently, retry
    indefinitely through outages, and are DLQ-replayable.
12. Audit records ride the local journal to platform audit ingest with
    before/after states; Sys outages delay rather than lose audit.
13. Rolling instance upgrades complete with graceful drain and no lost
    decisions: state lives in the database, the next cycle recomputes, and
    expiry settlement replays idempotently.
14. Admin/dashboards access margin data only through the Shield-gated,
    RLS-scoped API (query routes + RFW aliases), with platform-admin access
    preserved through the mirror's admin permits.

## 10. Open Business Questions

- Public event contract: the exact bus event names/payloads for margin calls,
  liquidations, funding, position updates, and expiry settlements (vocabulary
  fixed; envelope finalization pending with clearing/settlement consumers).
- Margin model policy: SPAN/portfolio margin for listed contracts versus the
  shipped isolated/cross basis-point model (listed-options/futures standard
  vs crypto-standard simplicity) — product decision pending.
- Liquidation policy mechanics: close-order retry windows, escalation rules,
  insurance-fund/ADL alternatives before forced market closes, and partial
  liquidation strategies.
- Funding interval and premium-index methodology per market (standard 8h
  cadence vs configurable; index composition owned by VFeeds but binding to
  contracts pending).
- Margin-call lifecycle rules: acknowledgement deadlines, escalation, and
  whether open margin calls block new opening trades in the pre-trade
  pipeline.
- Options expiry/exercise policy: auto-exercise threshold, assignment
  allocation (pro-rata vs random), and the exercise/assignment event
  vocabulary (Clearing-owned workflow).
- Settlement-style defaults per market segment (cash vs physical for equity
  futures; CCP-dependent) and the custody/CCP handoff contract for recorded
  delivery obligations.
- Greeks-cache cadence and implied-volatility surface sourcing (when more
  than flat/intra-contract IV is required).
- Fiat/custody margin rails: the deposit/withdrawal and funding-posting
  integrations with Bank8:Ledger and the crypto custody adapter.
- Anonymous vs attributed exposure in the margin views surfaced to brokers
  and clients (what account-level detail is disclosed, to whom) — disclosure
  policy pending final rule text.

---

**Design attribution:** exchange8:margin and its underlying concepts were designed by Hamid Jahanzad <Hamid@Jahanzad.com>.
