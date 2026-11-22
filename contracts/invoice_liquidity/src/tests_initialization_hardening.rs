#![cfg(test)]

//! Issue #1162 — deploy-time initialization hardening.
//!
//! Deployment (`scripts/deploy-testnet.sh`) installs the WASM, but
//! `initialize` is a *separate* invocation. These tests pin the two properties
//! that make that separation safe:
//!
//! 1. `initialize` requires the intended administrator to authorize the call,
//!    so a first-come caller cannot name itself admin.
//! 2. `initialize` is replay-safe: any explicit or legacy initialization
//!    signal makes a repeat call fail deterministically with
//!    `AlreadyInitialized`, including on an instance that was set up before
//!    this change (no `Initialized` marker yet).
//!
//! Authoritative invariants:
//! * INV-INIT1 — `initialize` is callable only by (an authorizer for) `admin`.
//! * INV-INIT2 — `Initialized` is written before any other state (CEI).
//! * INV-INIT3 — a second/competing `initialize` never mutates stored state.
//! * INV-INIT4 — legacy deployments (`Admin`/`Config` present, no marker)
//!   remain closed to re-initialization after an upgrade.

use super::*;
use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal,
};

/// Registers a fresh `invoice_liquidity` instance on an env that does **not**
/// mock authorizations, so `require_auth()` is genuinely enforced.
fn setup_no_auth() -> (
    Env,
    Address,
    Address,
    InvoiceLiquidityContractClient<'static>,
) {
    let env = Env::default();
    let admin = Address::generate(&env);
    // Reuse one address for all three bootstrap tokens; the values are
    // irrelevant to the authorization/guard behavior under test.
    let token = Address::generate(&env);

    let contract_id = env.register(InvoiceLiquidityContract, ());
    let client = InvoiceLiquidityContractClient::new(&env, &contract_id);

    (env, admin, token, client)
}

// ── Authorization (INV-INIT1) ────────────────────────────────────────────────

#[test]
fn initialize_requires_admin_authorization() {
    let (_env, admin, token, client) = setup_no_auth();

    // No auth mocked for `admin` → the call must not succeed.
    let res = client.try_initialize(&admin, &token, &token, &token);
    assert!(
        res.is_err(),
        "initialize must require the intended admin's authorization"
    );
}

#[test]
fn initialize_succeeds_when_admin_authorizes() {
    let (env, admin, token, client) = setup_no_auth();

    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &client.address,
            fn_name: "initialize",
            args: (admin.clone(), token.clone(), token.clone(), token.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let res = client.try_initialize(&admin, &token, &token, &token);
    assert!(
        res.is_ok(),
        "initialize should succeed when the admin authorizes: {res:?}"
    );

    // INV-INIT2/INV-INIT3: the marker and admin are recorded.
    env.as_contract(&client.address, || {
        assert!(
            env.storage().instance().has(&DataKey::Initialized),
            "initialize must persist the Initialized marker"
        );
        let stored: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert_eq!(stored, admin);
    });
}

// ── Replay safety / competing initializers (INV-INIT3) ───────────────────────

#[test]
fn repeated_initialize_fails_deterministically() {
    let (env, admin, token, client) = setup_no_auth();
    env.mock_all_auths();

    client.initialize(&admin, &token, &token, &token);

    let res = client.try_initialize(&admin, &token, &token, &token);
    assert_eq!(res, Err(Ok(ContractError::AlreadyInitialized)));
}

#[test]
fn competing_initializer_cannot_seize_admin() {
    let (env, admin, token, client) = setup_no_auth();
    env.mock_all_auths();

    client.initialize(&admin, &token, &token, &token);

    // A competing caller — even one that authorizes — cannot re-run initialize
    // nor displace the recorded admin.
    let attacker = Address::generate(&env);
    let res = client.try_initialize(&attacker, &token, &token, &token);
    assert_eq!(res, Err(Ok(ContractError::AlreadyInitialized)));

    env.as_contract(&client.address, || {
        let stored: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert_eq!(stored, admin, "admin must be unchanged by a rejected call");
    });
}

// ── Explicit marker vs. legacy state (INV-INIT2 / INV-INIT4) ─────────────────

#[test]
fn explicit_marker_alone_blocks_reinitialization() {
    let (env, admin, token, client) = setup_no_auth();
    env.mock_all_auths();

    // A partially-applied / already-started setup: only the marker exists.
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Initialized, &true);
    });

    let res = client.try_initialize(&admin, &token, &token, &token);
    assert_eq!(res, Err(Ok(ContractError::AlreadyInitialized)));
}

#[test]
fn legacy_initialized_deployment_cannot_be_reinitialized() {
    let (env, admin, token, client) = setup_no_auth();
    env.mock_all_auths();

    // Simulate a deployment initialized *before* #1162: `Admin` is present but
    // the dedicated `Initialized` marker is not. Upgrading such an instance
    // must not re-open initialization.
    env.as_contract(&client.address, || {
        env.storage().instance().set(&DataKey::Admin, &admin);
    });

    let res = client.try_initialize(&admin, &token, &token, &token);
    assert_eq!(res, Err(Ok(ContractError::AlreadyInitialized)));
}
