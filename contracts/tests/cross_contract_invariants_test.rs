//! Issue #1159 — cross-contract invoice lifecycle and accounting invariants.
//!
//! Proves that a failed lifecycle transaction leaves invoice, LP, insurance
//! and distribution accounting unchanged (rolled back atomically), and that
//! successful settlement / default are applied to every contract exactly
//! once. Uses the *real* `iln_distribution` and `insurance_pool` contracts
//! plus the shared `MockToken` (which can inject a transfer failure).
//!
//! Invariant IDs refer to `docs/invoice-lifecycle-invariants.md`.
//!
//! NOTE: `mock_all_auths()` (root-only) is used deliberately instead of
//! `mock_all_auths_allowing_non_root_auth()`: a distribution contract that
//! is wired to a *different* ILN address must reject ILN's nested
//! `accrue_*` call, exactly as it would on-chain.

extern crate std;

#[path = "mocks/mock_token.rs"]
mod mock_token;

use iln_distribution::{IlnDistribution, IlnDistributionClient};
use insurance_pool::{InsurancePool, InsurancePoolClient};
use invoice_liquidity::{
    ContractError, InvoiceLiquidityContract, InvoiceLiquidityContractClient, InvoiceStatus,
    ReferralCode,
};
use mock_token::{MockToken, MockTokenClient};
use soroban_sdk::{
    contracttype,
    testutils::{Address as _, EnvTestConfig, Ledger},
    Address, Env,
};

const AMOUNT: i128 = 1_000_000_000;
const RATE: u32 = 300;
const DAY: u64 = 24 * 60 * 60;
const T0: u64 = 1_700_000_000;
const SEQ0: u32 = 10_000;
const COVERAGE_CAP: i128 = 1_000;
const PREMIUM: i128 = 600;
/// ILN treasury reserve. `claim_default` and upheld disputes refund LP
/// principal that was already forwarded to the freelancer at funding time,
/// so those refunds are paid out of ILN's pooled balance, not a per-invoice
/// escrow (see [NE-2] in the invariants doc).
const RESERVE: i128 = 10 * AMOUNT;

/// Same storage key layout as `MockToken`'s private key enum, so a test can
/// disarm `fail_next_transfer`. Disarming is necessary because the flag's
/// one-shot `remove` is itself rolled back with the failed transaction —
/// which is precisely the atomicity property under test.
#[contracttype]
enum MockTokenKey {
    #[allow(dead_code)]
    Balance(Address),
    FailNext,
}

struct World {
    env: Env,
    iln: InvoiceLiquidityContractClient<'static>,
    token: MockTokenClient<'static>,
    dist: IlnDistributionClient<'static>,
    pool: InsurancePoolClient<'static>,
    admin: Address,
    freelancer: Address,
    payer: Address,
    lp: Address,
    lp2: Address,
}

/// Everything a failed transaction must leave untouched.
#[derive(Debug, PartialEq)]
struct Ledgers {
    invoice: invoice_liquidity::Invoice,
    lp_index: u32,
    total_funded: u64,
    total_paid: u64,
    payer_score: u32,
    balances: std::vec::Vec<i128>,
    pool_balance: i128,
    pool_claimed: bool,
    accruals: std::vec::Vec<i128>,
}

fn advance(env: &Env, secs: u64, ledgers: u32) {
    let mut li = env.ledger().get();
    li.timestamp += secs;
    li.sequence_number += ledgers;
    env.ledger().set(li);
}

fn world() -> World {
    world_with_reserve(RESERVE)
}

fn world_with_reserve(reserve: i128) -> World {
    let env = Env::new_with_config(EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    env.mock_all_auths();
    let mut li = env.ledger().get();
    li.timestamp = T0;
    li.sequence_number = SEQ0;
    env.ledger().set(li);

    let admin = Address::generate(&env);
    let freelancer = Address::generate(&env);
    let payer = Address::generate(&env);
    let lp = Address::generate(&env);
    let lp2 = Address::generate(&env);

    let token_id = env.register(MockToken, ());
    let token = MockTokenClient::new(&env, &token_id);
    for a in [&lp, &lp2, &payer] {
        token.mint(a, &(10 * AMOUNT));
    }

    let xlm = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let iln_id = env.register(InvoiceLiquidityContract, ());
    let iln = InvoiceLiquidityContractClient::new(&env, &iln_id);
    iln.initialize(&admin, &token_id, &Address::generate(&env), &xlm);
    if reserve > 0 {
        token.mint(&iln_id, &reserve);
    }

    let gov = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let dist_id = env.register(IlnDistribution, ());
    let dist = IlnDistributionClient::new(&env, &dist_id);
    dist.initialize(&iln_id, &gov);
    iln.set_distribution_contract(&dist_id);

    let pool_id = env.register(InsurancePool, ());
    let pool = InsurancePoolClient::new(&env, &pool_id);
    pool.initialize(&iln_id, &COVERAGE_CAP, &token_id);
    iln.set_insurance_pool(&pool_id);

    World {
        env,
        iln,
        token,
        dist,
        pool,
        admin,
        freelancer,
        payer,
        lp,
        lp2,
    }
}

impl World {
    fn submit(&self) -> u64 {
        self.iln.submit_invoice(
            &self.freelancer,
            &self.payer,
            &AMOUNT,
            &(T0 + 30 * DAY),
            &RATE,
            &self.token.address,
            &ReferralCode::None,
        )
    }

    fn actors(&self) -> [Address; 7] {
        [
            self.admin.clone(),
            self.freelancer.clone(),
            self.payer.clone(),
            self.lp.clone(),
            self.lp2.clone(),
            self.iln.address.clone(),
            self.pool.address.clone(),
        ]
    }

    fn ledgers(&self, id: u64) -> Ledgers {
        let stats = self.iln.get_contract_stats();
        Ledgers {
            invoice: self.iln.get_invoice(&id),
            lp_index: self.iln.list_invoices_by_lp(&self.lp, &0, &50).len(),
            total_funded: stats.total_funded,
            total_paid: stats.total_paid,
            payer_score: self.iln.payer_score(&self.payer),
            balances: self
                .actors()
                .iter()
                .map(|a| self.token.balance(a))
                .collect(),
            pool_balance: self.pool.get_pool_balance(),
            pool_claimed: self.pool.is_claimed(&id),
            accruals: [&self.lp, &self.lp2, &self.freelancer, &self.payer]
                .iter()
                .map(|a| self.dist.get_accrual(a))
                .collect(),
        }
    }

    fn total_tokens(&self) -> i128 {
        self.actors().iter().map(|a| self.token.balance(a)).sum()
    }

    /// Re-wire ILN to a distribution contract (admin call, rate-limited).
    fn rewire_distribution(&self, dist: &Address) {
        advance(&self.env, 0, 200);
        self.iln.set_distribution_contract(dist);
    }

    /// A real distribution contract that trusts a *different* ILN address,
    /// so every `accrue_*` call from this ILN fails authorization.
    fn miswired_distribution(&self) -> Address {
        let gov = self
            .env
            .register_stellar_asset_contract_v2(Address::generate(&self.env))
            .address();
        let id = self.env.register(IlnDistribution, ());
        IlnDistributionClient::new(&self.env, &id).initialize(&Address::generate(&self.env), &gov);
        id
    }

    fn disarm_token_failure(&self) {
        let env = &self.env;
        env.as_contract(&self.token.address, || {
            env.storage().temporary().remove(&MockTokenKey::FailNext)
        });
    }
}

/// [RB-2] Funding: the LP->ILN and ILN->freelancer token transfers have
/// already executed when the distribution hook fails; all of it must revert.
#[test]
fn xc_funding_rolls_back_when_distribution_accrual_fails() {
    let w = world();
    let bad = w.miswired_distribution();
    w.rewire_distribution(&bad);
    let id = w.submit();

    let before = w.ledgers(id);
    for amt in [AMOUNT / 2, AMOUNT] {
        assert!(w.iln.try_fund_invoice(&w.lp, &id, &amt, &false).is_err());
        assert_eq!(w.ledgers(id), before, "fund {amt}: nothing may persist");
    }
    assert_eq!(before.invoice.status, InvoiceStatus::Pending);
    assert_eq!(before.invoice.amount_funded, 0);
}

/// [RB-2] Settlement: payer->ILN, ILN->LP and fee transfers, status=Paid
/// and reputation updates all revert if the distribution settlement hook
/// fails; once the hook works, settlement is applied (and accrued) once.
#[test]
fn xc_settlement_rolls_back_when_distribution_settlement_fails() {
    let w = world();
    let good = w.dist.address.clone();
    let id = w.submit();
    w.iln.fund_invoice(&w.lp, &id, &AMOUNT, &false);
    assert!(w.dist.get_accrual(&w.lp) > 0);

    let bad = w.miswired_distribution();
    w.rewire_distribution(&bad);
    let before = w.ledgers(id);
    assert!(w.iln.try_mark_paid(&id, &AMOUNT).is_err());
    assert_eq!(w.ledgers(id), before);
    assert_eq!(before.invoice.status, InvoiceStatus::Funded);
    assert_eq!(before.invoice.amount_paid, 0);

    // Partial payments do not call the settlement hook, so they succeed.
    w.iln.mark_paid(&id, &(AMOUNT / 2));
    let partial = w.ledgers(id);
    assert_eq!(partial.invoice.amount_paid, AMOUNT / 2);
    assert!(w.iln.try_mark_paid(&id, &(AMOUNT / 2)).is_err());
    assert_eq!(w.ledgers(id), partial);

    w.rewire_distribution(&good);
    let freelancer_accrual = w.dist.get_accrual(&w.freelancer);
    w.iln.mark_paid(&id, &(AMOUNT / 2));
    let settled = w.ledgers(id);
    assert_eq!(settled.invoice.status, InvoiceStatus::Paid);
    assert_eq!(settled.total_paid, before.total_paid + 1);
    assert!(w.dist.get_accrual(&w.freelancer) > freelancer_accrual);

    // [SET-1] settlement cannot be applied (or accrued) a second time.
    assert_eq!(
        w.iln.try_mark_paid(&id, &1),
        Err(Ok(ContractError::AlreadyPaid))
    );
    assert_eq!(w.ledgers(id), settled);
}

/// [RB-2] A token transfer failure aborts funding with no trace in ILN,
/// LP balances or distribution accruals.
#[test]
fn xc_token_failure_rolls_back_funding_and_accrual() {
    let w = world();
    let id = w.submit();
    let before = w.ledgers(id);

    // Partial and full tranches alike leave no trace when a transfer fails.
    for amt in [AMOUNT / 4, AMOUNT] {
        w.token.fail_next_transfer();
        assert!(w.iln.try_fund_invoice(&w.lp, &id, &amt, &false).is_err());
        assert_eq!(w.ledgers(id), before, "fund {amt}");
        w.disarm_token_failure();
    }

    // (Distribution accrues per whole 100-USDC of volume, so fund the full
    // 100-USDC invoice in one tranche to make the accrual observable.)
    w.iln.fund_invoice(&w.lp, &id, &AMOUNT, &false);
    let after = w.ledgers(id);
    assert_eq!(after.invoice.status, InvoiceStatus::Funded);
    assert_eq!(after.invoice.amount_funded, AMOUNT);
    assert_eq!(after.lp_index, 1);
    assert!(
        after.accruals[0] > before.accruals[0],
        "accrual only on success"
    );
}

/// [RB-3] `claim_default` refunds the LP and then claims insurance. If the
/// refund transfer fails, the insurance pool must not have paid out or
/// marked the invoice claimed, and the invoice must still be Funded.
/// Once it succeeds, the insurance payout happens exactly once.
#[test]
fn xc_failed_default_leaves_insurance_untouched_and_success_is_once() {
    let w = world();
    w.token.mint(&w.lp, &PREMIUM);
    w.pool.deposit_premium(&w.lp, &PREMIUM);
    assert!(w.pool.is_enrolled(&w.lp));

    let id = w.submit();
    w.iln.fund_invoice(&w.lp, &id, &AMOUNT, &false);
    advance(&w.env, 31 * DAY, 100);

    let before = w.ledgers(id);
    w.token.fail_next_transfer();
    assert!(w.iln.try_claim_default(&w.lp, &id).is_err());
    assert_eq!(
        w.ledgers(id),
        before,
        "no partial default / insurance state"
    );
    w.disarm_token_failure();

    let supply = w.total_tokens();
    let lp_before = w.token.balance(&w.lp);
    w.iln.claim_default(&w.lp, &id);
    let after = w.ledgers(id);
    assert_eq!(after.invoice.status, InvoiceStatus::Defaulted);
    assert!(after.pool_claimed);
    let payout = before.pool_balance - after.pool_balance;
    assert!(payout > 0 && payout <= COVERAGE_CAP);
    assert_eq!(
        w.token.balance(&w.lp) - lp_before,
        AMOUNT - AMOUNT * RATE as i128 / 10_000 + payout,
        "LP gets principal refund + insurance payout, once"
    );
    assert_eq!(
        w.total_tokens(),
        supply,
        "[ACC-6] tokens conserved across ILN + pool"
    );

    // [SET-2] a second default claim changes nothing in either contract.
    assert_eq!(
        w.iln.try_claim_default(&w.lp, &id),
        Err(Ok(ContractError::InvoiceDefaulted))
    );
    assert_eq!(w.ledgers(id), after);
}

/// An insurance payout failure (empty pool) is tolerated by design: the
/// default still completes, and the pool's accounting is untouched (no
/// partial claim marker, no balance change).
#[test]
fn xc_insurance_failure_does_not_corrupt_pool_or_block_default() {
    let w = world();
    w.pool.enroll(&w.lp); // enrolled but the pool holds no reserve
    let id = w.submit();
    w.iln.fund_invoice(&w.lp, &id, &AMOUNT, &false);
    advance(&w.env, 31 * DAY, 100);

    let before = w.ledgers(id);
    w.iln.claim_default(&w.lp, &id);
    let after = w.ledgers(id);
    assert_eq!(after.invoice.status, InvoiceStatus::Defaulted);
    assert_eq!(after.pool_balance, before.pool_balance);
    assert!(
        !after.pool_claimed,
        "failed insurance claim leaves no marker"
    );
    assert_eq!(
        after.accruals, before.accruals,
        "default accrues no rewards"
    );
}

/// Upheld dispute on a Funded invoice draws LP refunds from ILN's own
/// balance ([NE-2]). With an insufficient balance, the resolution reverts
/// atomically, including the distribution/insurance side.
#[test]
fn xc_insolvent_dispute_resolution_reverts_atomically() {
    let w = world_with_reserve(0);
    let id = w.submit();
    w.iln.fund_invoice(&w.lp, &id, &AMOUNT, &false);
    w.iln
        .dispute_invoice(&id, &soroban_sdk::BytesN::from_array(&w.env, &[1; 32]));
    let before = w.ledgers(id);
    assert!(w
        .iln
        .try_resolve_dispute(&id, &soroban_sdk::BytesN::from_array(&w.env, &[2; 32]), &1)
        .is_err());
    assert_eq!(w.ledgers(id), before);
    assert_eq!(before.invoice.status, InvoiceStatus::Disputed);
}

/// End-to-end: one invoice settled, one defaulted with insurance, a failed
/// funding attempt in between. Token supply is conserved across every
/// contract and distribution accrues only for successful operations.
#[test]
fn xc_end_to_end_accounting_is_conserved() {
    let w = world();
    w.token.mint(&w.lp, &PREMIUM);
    w.pool.deposit_premium(&w.lp, &PREMIUM);
    let supply = w.total_tokens();

    let a = w.submit();
    w.iln.fund_invoice(&w.lp, &a, &AMOUNT, &false);
    let accrual_one = w.dist.get_accrual(&w.lp);
    assert!(accrual_one > 0);

    let b = w.submit();
    assert_eq!(
        w.iln.try_fund_invoice(&w.lp, &b, &(AMOUNT + 1), &false),
        Err(Ok(ContractError::OverfundingRejected))
    );
    assert_eq!(w.dist.get_accrual(&w.lp), accrual_one);
    w.iln.fund_invoice(&w.lp, &b, &AMOUNT, &false);
    assert_eq!(w.dist.get_accrual(&w.lp), 2 * accrual_one);

    w.iln.mark_paid(&a, &AMOUNT);
    advance(&w.env, 31 * DAY, 100);
    w.iln.claim_default(&w.lp, &b);

    assert_eq!(w.iln.get_invoice(&a).status, InvoiceStatus::Paid);
    assert_eq!(w.iln.get_invoice(&b).status, InvoiceStatus::Defaulted);
    assert!(w.pool.is_claimed(&b) && !w.pool.is_claimed(&a));
    assert_eq!(w.total_tokens(), supply);
    assert!(w.token.balance(&w.iln.address) >= 0);
    let stats = w.iln.get_contract_stats();
    assert_eq!((stats.total_funded, stats.total_paid), (2, 1));
}
