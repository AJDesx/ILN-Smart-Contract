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
| C1 | Cross-contract spec X6: token precision must be known wherever amounts are compared | `validate_invoice_terms_with_token` used `.unwrap_or(6)`; `initialize` allowlisted EURC without writing decimals, so EURC (and any legacy token) silently got the 6-decimal floor | `lib.rs` `validate_invoice_terms_with_token` → `TokenDecimalsMissing`; `initialize` writes EURC=6 | `submit_invoice` / batch submit revert, no invoice id consumed | `c1_*` |
| C2 | Same | `add_token` accepted any `u32` decimals; `ten_pow` saturates, so a huge value produced a nonsensical floor | `add_token` rejects `decimals > MAX_TOKEN_DECIMALS (18)` with `InvalidTokenDecimals`, after `require_admin` | Admin tx reverts, no state written | `c2_*` |
| C3 | Cross-contract spec X7 / formal-verification §5 (bounded state) | `MAX_FUND_QUEUE_LEN`, `QueueFull`, `FundQueueEntryDisplaced` were declared but **never enforced**: `join_fund_queue` grew `FundQueue(invoice_id)` without bound (unbounded storage + O(n) duplicate scan and insert) | `join_fund_queue`: at capacity, joiner must strictly outscore the tail, which is displaced (`fund_queue_displaced` event); else `QueueFull` | Equal/lower score join reverts; queue length never exceeds 50 | `c3_*` |
| C4 | `cross_contract_invariants_test.rs` X1–X5 | Harness is the only cross-contract regression; it needs wired distribution + insurance | Left unchanged (see §4) | n/a | existing `x1`–`x5` |

Authorization: no new entry points. `add_token` keeps `require_admin` as its first
check; `join_fund_queue` keeps `require_lp` (LP auth). No path gained a bypass.

## 2. Before / after (measured)

Same `audit_regression_test` suite, run against the baseline `lib.rs` and the
fixed `lib.rs`:

| | Baseline | After |
|---|---|---|
| Tests passing | 4 / 11 | 11 / 11 |
| Failing baseline tests | `c1_initialize_registers_decimals…`, `c1_missing_decimals_fails_closed…`, `c2_add_token_rejects…`, 4 × `c3_*` | none |
| Max fund-queue length reachable | unbounded | `MAX_FUND_QUEUE_LEN` = 50 |
| Behaviour with missing decimals | accepts using 6 | `TokenDecimalsMissing` |

(The 4 baseline passes are happy-path / unrelated-rejection guards that must
keep passing: unlisted token still `Unauthorized`, normal submit still works,
exactly-50 joins still accepted, a full queue still resolves.)

## 3. Rollout, recovery and upgrade

**Storage / ABI.** No storage layout change. Two new `ContractError` codes
(60, 61) are appended; existing codes are untouched. The queue cap re-uses the
existing `FundQueue`, `FundQueueEntryDisplaced` and `QueueFull` definitions.

**Pre-upgrade check (operators).** For every allowlisted token (the `TokenList`), call `get_token_decimals(token)`. Any `None` will start
reverting new invoice submissions after the upgrade. Mainnet deployments
initialised before this change will have `None` for **EURC**.

**Backfill.** Before or immediately after upgrading, an admin (or governance
`AddToken` proposal) calls `add_token(eurc, 6)`. Note `add_token` is
rate-limited and performs the fee-on-transfer probe (the admin must hold the
probe amount of that token). Existing invoices are unaffected: decimals are only
read at submission time.

**Queues already above 50.** Existing oversized queues are not truncated.
New joins to such a queue are rejected unless they outscore the tail (then the tail is
displaced and the length stays the same). Queues
drain as invoices resolve, are funded or are cancelled.

**Recovery / rollback.** Reverting the Wasm restores prior behaviour with no
data migration, because no state was rewritten. If `TokenDecimalsMissing`
blocks a token in production, the fix is `add_token` backfill, not a rollback.

**Monitoring.** Alert on `fund_queue_displaced` (queue pressure / griefing
signal) and on `TokenDecimalsMissing` / `QueueFull` error rates.

## 4. Known limits (not addressed here)

- `cross_contract_invariants_test.rs` currently aborts in `Harness::new`
  (`WasmVm InvalidAction`) on the base branch, so X1–X5 do not run today. New
  X6/X7 coverage therefore lives in the self-contained
  `audit_regression_test.rs`.
- `ContractError` codes 47–49 (`QueueFull`, `QueueUnresolved`,
  `PriceQuorumNotMet`) collide with the numbers `multisig.rs` expects for its
  variants, and the base branch does not compile for that reason. This is
  pre-existing and out of scope; the new codes start at 60 so they do not add
  to the collision.
- The in-crate unit-test modules (`src/tests_*.rs`) do not compile on the base
  branch for unrelated reasons (private test fns, missing variants), which is
  why the suite is an integration target.
