# Initialization Hardening (Issue #1162)

This document describes how `invoice_liquidity` initialization is authorized,
why it is safe against replay and competing first callers, and how operators
should recover from a failed or interrupted initialization.

---

## 1. Background

Soroban separates **WASM deployment** (`stellar contract deploy`) from
**initialization** (`initialize`). Deployment installs the instance; a separate
transaction must then establish the privileged `admin` address.

Before #1162, `initialize(env, admin, usdc_token, eurc_token, xlm_token)` in
[lib.rs](../contracts/invoice_liquidity/src/lib.rs) had two weaknesses:

1. It never called `admin.require_auth()`, so **anyone** could be the first
   caller.
2. It inferred "already initialized" from the presence of `DataKey::InvoiceCount`
   alone — an optional-looking signal that a partially set-up instance might not
   have.

Because the admin controls fees, approved tokens, oracle settings, pause state,
upgrades, migrations, and other governance-integrated operations, an
unauthorized first caller could seize the instance during the public window
between deployment and initialization.

---

## 2. Authenticated initialization flow

`initialize` now **requires the supplied `admin` to authorize the call**:

```rust
if crate::storage::is_initialized(&env) {
    return Err(ContractError::AlreadyInitialized);
}

// The intended initializer must authorize the call.
admin.require_auth();

// Checks-effects-interactions: write the marker first.
env.storage()
    .instance()
    .set(&crate::storage::DataKey::Initialized, &true);
env.storage()
    .instance()
    .set(&crate::storage::DataKey::Admin, &admin);
```

Consequences:

- The `admin` argument **must be the same identity that signs the transaction**.
- A deployer, CI key, or any other key **cannot** authorize initialization on
  behalf of a *different* administrator. If the intended admin is not the
  deployer, that admin must submit the `initialize` transaction themselves
  (e.g. `stellar contract invoke --source <admin-key> ... -- initialize ...`).

### Deployer vs. administrator

| Scenario | Whoever may call `initialize` |
|----------|-------------------------------|
| Admin == deployer | The deployer (single signed transaction) |
| Admin != deployer | The admin identity only — the deployer cannot authorize for it |

`scripts/deploy-testnet.sh` handles both: when `INVOICE_LIQUIDITY_ADMIN`
matches the deployer it initializes automatically; otherwise it prints the exact
`stellar contract invoke --source <admin-key>` command to run manually.

---

## 3. Replay safety and competing initializers

Initialization is **one-shot and fail-closed**:

- **Authoritative marker.** `DataKey::Initialized` is written *before* any other
  state (checks-effects-interactions), so once setup begins the instance is
  already marked and a competing call fails deterministically.
- **Legacy signal detection.** `storage::is_initialized` additionally treats the
  historical `Admin` / `Config` / `NextInvoiceId` / `InvoiceCount` keys as
  "already initialized". An instance created before #1162 therefore **cannot be
  re-initialized after an upgrade**, closing the takeover path on existing
  deployments.
- **Deterministic failure.** Any repeated or competing call returns
  `ContractError::AlreadyInitialized` (code `16`) rather than silently
  succeeding or partially overwriting admin.

The `Initialized` key is **appended last** in the `DataKey` enum on purpose:
`#[contracttype]` encodes variants by position, so inserting it earlier would
renumber every following key and corrupt existing storage. See
[storage-layout.md](./storage-layout.md).

### Invariants

| ID | Invariant |
|----|-----------|
| INV-INIT1 | `initialize` succeeds only when `admin.require_auth()` passes. |
| INV-INIT2 | After a successful `initialize`, a second call returns `AlreadyInitialized`. |
| INV-INIT3 | A competing caller (different signer/address) can never change the stored `Admin`. |
| INV-INIT4 | Any pre-existing initialization signal (marker **or** legacy key) blocks re-initialization. |

---

## 4. Failure recovery

| Situation | Symptom | Recovery |
|-----------|---------|----------|
| Init transaction rejected (auth) | `Unauthorized` | Re-run `initialize` **from the intended admin key**. |
| Init ran but the app never recorded success | Chain shows `AlreadyInitialized` on retry | Initialization **already succeeded**; no action needed — verify via `get_admin`. |
| Partial/reverted setup | Transaction reverted | Nothing was committed (Soroban transactions are atomic); re-run `initialize`. |
| Wrong admin was set (pre-#1162 instance) | `get_admin` returns an unexpected address | Rotate admin via the governance/admin flow (see [multisig-admin-runbook.md](./multisig-admin-runbook.md)); `initialize` cannot re-run. |
| Admin identity differs from deployer | Deployer-init fails with auth error | The admin must submit `initialize` themselves (see §2). |

Because the transaction is atomic, there is no partially-initialized on-chain
state to clean up: a failed `initialize` leaves the instance exactly as it was.

---

## 5. Operational scripts

- `scripts/deploy-testnet.sh` — performs the **authenticated** `initialize`
  step immediately after deployment (tolerating `AlreadyInitialized` as a
  no-op), or prints the manual command when admin ≠ deployer.
- `scripts/seed.ts` — initializes only if `get_invoice_count` is unavailable;
  its admin argument is the same key that signs the transaction.
- `scripts/smoke-test.ts` — same pattern; the `admin` argument equals the
  signing key.

---

## 6. Tests

Coverage lives in
[tests_initialization_hardening.rs](../contracts/invoice_liquidity/src/tests_initialization_hardening.rs):

- `initialize_requires_admin_authorization`
- `initialize_succeeds_when_admin_authorizes`
- `repeated_initialize_fails_deterministically`
- `competing_initializer_cannot_seize_admin`
- `explicit_marker_alone_blocks_reinitialization`
- `legacy_initialized_deployment_cannot_be_reinitialized`

---

## 7. References

- Issue #1162 — Secure deploy-time initialization and prevent unauthorized admin selection
- [storage-layout.md](./storage-layout.md)
- [upgrade-guide.md](./upgrade-guide.md)
- [multisig-admin-runbook.md](./multisig-admin-runbook.md)
