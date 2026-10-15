pub const MAX_DISCOUNT_RATE: u32 = 5000;
pub const DEFAULT_PAYER_SCORE: u32 = 50;
pub const DEFAULT_LP_SCORE: u32 = 50;
pub const TOP_PAYERS_CAPACITY: u32 = 50;
pub const CONTRACT_VERSION: &str = "1.0.0";
/// Issue #539: Current storage schema version. Bump this whenever the
/// persistent/instance storage layout changes so that `migrate()` can
/// detect and apply incremental upgrades.
pub const CURRENT_STORAGE_VERSION: u32 = 2;

// ----------------------------------------------------------------
// Rate Limiting Defaults (Issue #541)
// ----------------------------------------------------------------

/// Default minimum delay between sensitive admin operations in ledgers.
/// At ~5 seconds per ledger, 120 ledgers ≈ 10 minutes.
pub const DEFAULT_RATE_LIMIT_LEDGERS: u64 = 120;

/// Rate limit cooldown for `set_admin` — 1 hour (720 ledgers at 5s).
pub const ADMIN_CHANGE_COOLDOWN_LEDGERS: u64 = 720;

/// Rate limit cooldown for `upgrade` — 2 hours (1440 ledgers at 5s).
pub const UPGRADE_COOLDOWN_LEDGERS: u64 = 1440;

/// Rate limit cooldown for economic parameters — 30 minutes (360 ledgers).
pub const ECONOMIC_PARAM_COOLDOWN_LEDGERS: u64 = 360;

// ----------------------------------------------------------------
// Reputation Decay Bounds (Issue #601)
// ----------------------------------------------------------------

/// Maximum number of decay periods `get_payer_score` will iterate before
/// short-circuiting the score to zero. See invoice.rs for full rationale.
pub const MAX_REPUTATION_DECAY_PERIODS: u64 = 1000;

/// Minimum number of ledgers that must elapse between the first LP joining the
/// fund queue and `resolve_fund_queue` being callable.  At ~5 s per ledger,
/// 120 ledgers ≈ 10 minutes, giving other LPs a fair window to join.
///
/// Prevents MEV / front-running: an attacker who observes a high-reputation LP
/// joining the queue can no longer immediately resolve the queue in the same
/// block to lock-out competing LPs (Issue #MEV-1).
pub const QUEUE_DELAY_LEDGERS: u32 = 120;

/// Issue #1091: maximum number of entries a single invoice's fund queue may
/// hold. `join_fund_queue` is O(n) in the queue length (duplicate scan +
/// sorted insert) and rewrites the whole vector, so an unbounded queue lets
/// anyone grief the queue (and, since Issue #1090, funding of the invoice)
/// by spamming joins from fresh addresses until the entry exceeds Soroban's
/// per-transaction resource limits.
///
/// When the queue is full a new LP is admitted only if its score strictly
/// exceeds the lowest queued score, displacing that lowest entry — an entry
/// that can no longer win, so every *potential* winner is retained until
/// resolution.
pub const MAX_FUND_QUEUE_LEN: u32 = 50;

/// Issue #1091: TTL policy for the fund-queue keys (`FundQueue`,
/// `FundQueueOpenedAt`, `QueueResolution`). Matches the invoice's own
/// `extend_ttl(1_000_000, 2_000_000)` in `storage::save_invoice` so the
/// queue cannot be archived while the invoice it gates is still live.
pub const FUND_QUEUE_TTL_THRESHOLD: u32 = 1_000_000;
pub const FUND_QUEUE_TTL_EXTEND_TO: u32 = 2_000_000;

/// Issue #1082: minimum remaining TTL (in ledgers) that every live persistent
/// storage entry must have before an upgrade is permitted.  At ~5 s per
/// ledger, 500_000 ledgers ≈ 29 days — long enough to safely complete a
/// migration without any data expiring mid-flight.
///
/// The pre-upgrade guard samples `InvoiceCore` / `InvoiceMetadata` for the
/// most-recent `TTL_UPGRADE_SAMPLE_SIZE` invoices plus every live
/// `FundQueue` / `FundQueueOpenedAt` / `QueueResolution` entry it can find
/// in that same range. If *any* sampled entry's remaining TTL falls below
/// this threshold the upgrade is rejected fail-closed, forcing the admin to
/// refresh TTLs before retrying.
pub const TTL_MIN_LEDGERS_BEFORE_UPGRADE: u32 = 500_000;

/// Issue #1082: number of the most-recent invoice IDs to sample during the
/// pre-upgrade TTL check.  Bounded to cap the resource cost of the check;
/// the most-recently-written invoices are the most likely to still be
/// active.
pub const TTL_UPGRADE_SAMPLE_SIZE: u64 = 50;

/// Issue #1083: number of the most-recent invoice IDs sampled during a
/// rollback rehearsal.  Mirrors `TTL_UPGRADE_SAMPLE_SIZE` — the same
/// population that was TTL-checked before the upgrade is also checked for
/// storage-completeness before a rollback.  Bounded to keep the check
/// within per-transaction resource limits.
pub const ROLLBACK_SAMPLE_SIZE: u64 = 50;
