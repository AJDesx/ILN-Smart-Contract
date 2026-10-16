# Audit Evidence → Remediation Control Map

Issues #1094, #1095, #1096, #1097 (wave-9, `formal`).

Baseline evidence:

- [`formal-verification.md`](formal-verification.md)
- [`formal-verification-cross-contract.md`](formal-verification-cross-contract.md)
- `contracts/tests/cross_contract_invariants_test.rs`

Regression harness: `contracts/tests/audit_regression_test.rs`
(`cargo test -p invoice_liquidity --test audit_regression_test`).

## 1. Control map

| ID | Audit evidence | Gap found at baseline | Control (code) | Fail-closed behaviour | Test(s) |
|----|----------------|-----------------------|----------------|-----------------------|---------|
| C1 | Cross-contract spec X6: token precision must be known wherever amounts are compared | `validate_invoice_terms_with_token` used `.unwrap_or(6)`; `initialize` allowlisted EURC without decimals | **Landed upstream in #1070** (`token_safety.rs`, `TokenDecimalsNotConfigured`, EURC=6 at `initialize`, `migrate` backfill). This PR adds regression guards only | `submit_invoice` reverts, no invoice id consumed | `c1_*` |
| C2 | Same | `add_token` trusted the admin-supplied `decimals` | **Landed upstream in #1070** (declared decimals must equal the token's `decimals()` and be <= 18, `InvalidTokenDecimals`). Covered by `tests_token_safety.rs` | Admin tx reverts | (upstream) |
| C3 | Cross-contract spec X7 / formal-verification §5 (bounded state) | `MAX_FUND_QUEUE_LEN`, `QueueFull`, `FundQueueEntryDisplaced` were declared but **never enforced**: `join_fund_queue` grew `FundQueue(invoice_id)` without bound (unbounded storage + O(n) duplicate scan and insert) | `join_fund_queue`: at capacity, joiner must strictly outscore the tail, which is displaced (`fund_queue_displaced` event); else `QueueFull` | Equal/lower score join reverts; queue length never exceeds 50 | `c3_*` |
| C4 | `cross_contract_invariants_test.rs` X1–X5 | Harness is the only cross-contract regression; it needs wired distribution + insurance | Left unchanged (see §4) | n/a | existing `x1`–`x5` |

Authorization: no new entry points. `add_token` keeps `require_admin` as its first
check; `join_fund_queue` keeps `require_lp` (LP auth). No path gained a bypass.

## 2. Before / after (measured)

Same `audit_regression_test` suite (10 tests), run against `origin/dev`'s
`lib.rs` and the fixed `lib.rs`:

| | `origin/dev` | This PR |
|---|---|---|
| Tests passing | 6 / 10 | 10 / 10 |
| Failing tests | 4 × `c3_*` (equal-score, lower-score, higher-score displacement, displaced-LP rejoin) | none |
| Max fund-queue length reachable | unbounded | `MAX_FUND_QUEUE_LEN` = 50 |
| Missing token decimals | `TokenDecimalsNotConfigured` (from #1070; guarded by `c1_*`) | unchanged |

(The 6 baseline passes are guards that must keep passing: decimals registered,
missing decimals fail closed, unlisted token still `Unauthorized`, normal
submit works, exactly-50 joins accepted, a full queue still resolves.)

## 3. Rollout, recovery and upgrade

**Storage / ABI.** No storage layout change and no new error codes. The queue
cap re-uses the existing `FundQueue`, `FundQueueEntryDisplaced` and `QueueFull`
definitions. Decimals rollout/migration is documented in
[`token-safety.md`](token-safety.md) (#1070).

**Queues already above 50.** Existing oversized queues are not truncated.
New joins to such a queue are rejected unless they outscore the tail (then the
tail is displaced and the length stays the same). Queues drain as invoices
resolve, are funded or are cancelled.

**Recovery / rollback.** Reverting the Wasm restores prior behaviour with no
data migration, because no state was rewritten.

**Monitoring.** Alert on `fund_queue_displaced` (queue pressure / griefing
signal) and on `QueueFull` error rates.

## 4. Known limits (not addressed here)

- `cross_contract_invariants_test.rs` aborts in `Harness::new`
  (`WasmVm InvalidAction`) on the base branch, so X1–X5 do not run today. X6/X7
  coverage therefore lives in the self-contained `audit_regression_test.rs`.
- At the time of this PR `origin/dev` does not build `insurance_pool` /
  `iln_distribution` (duplicate `DataKey` variants; `initialize` return type),
  which blocks every `invoice_liquidity` integration target. Out of scope.
