#![cfg(test)]
extern crate std;

// Period billing statement test suite.
//
// Covers:
//  - write + read round-trip for PeriodBillingStatement
//  - idempotent upsert (overwrite same period_index)
//  - subscription index pagination (oldest-first slice)
//  - merchant time-range filter + pagination
//  - mixed subscriptions / merchants don't bleed across indices
//  - empty-state queries return empty vecs
//  - security: finalize_billing_statement requires admin auth
//  - status_flags bit constants round-trip through storage
//  - period statements coexist with per-charge statements (statements.rs)
//  - retention + compaction of per-charge rows does NOT affect period statements
//  - high-volume: 20 periods for one subscription, full paginated walk
//  - accounting: period amounts match corresponding charge audit row

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};

// T0 must be larger than INTERVAL to avoid u64 underflow in period_start_timestamp.
const T0: u64 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days ≈ 2_592_000 s

// ─── Minimal test environment ─────────────────────────────────────────────────

struct BillingTestEnv {
    env: Env,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    token: Address,
}

impl BillingTestEnv {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = T0);

        let admin = Address::generate(&env);
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(&env, &contract_id);

        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

        Self { env, client, admin, token }
    }

    fn mint(&self, to: &Address, amount: &i128) {
        soroban_sdk::token::StellarAssetClient::new(&self.env, &self.token).mint(to, amount);
    }

    fn make_subscription(&self, subscriber: &Address, merchant: &Address) -> u32 {
        self.client.create_subscription(
            subscriber,
            merchant,
            &1_000_000i128,
            &INTERVAL,
            &false,
            &None::<i128>,
            &None::<u64>,
        )
    }

    /// Write a period statement via `finalize_billing_statement`.
    ///
    /// `period_end_timestamp` is taken from the current ledger; `period_start` is
    /// one INTERVAL before. Avoids callers having to pass raw timestamps everywhere.
    fn finalize(
        &self,
        subscription_id: u32,
        period_index: u32,
        subscriber: &Address,
        merchant: &Address,
        total_charged: i128,
        finalized_by: BillingStatementFinalization,
    ) {
        let t_end = self.env.ledger().timestamp();
        let t_start = t_end - INTERVAL;
        self.client.finalize_billing_statement(
            &self.admin,
            &subscription_id,
            &period_index,
            subscriber,
            merchant,
            &t_start,
            &t_end,
            &PeriodStatementAmounts {
                total_amount_charged: total_charged,
                total_usage_units: 0,
                protocol_fee_amount: 0,
                net_amount_to_merchant: total_charged,
                refund_amount: 0,
            },
            &STMT_FLAG_INTERVAL_CHARGED,
            &finalized_by,
        );
    }
}

// ─── Round-trip ───────────────────────────────────────────────────────────────

#[test]
fn test_billing_statement_write_and_read() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);

    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.subscription_id, sub_id);
    assert_eq!(stmt.period_index, 0);
    assert_eq!(stmt.total_amount_charged, 1_000_000);
    assert_eq!(stmt.finalized_by, BillingStatementFinalization::PeriodClosed);
    assert_eq!(stmt.subscriber, subscriber);
    assert_eq!(stmt.merchant, merchant);
    assert_eq!(stmt.token, te.token);
    assert_eq!(stmt.status_flags, STMT_FLAG_INTERVAL_CHARGED);
    assert_eq!(stmt.period_start_timestamp, T0);
    assert_eq!(stmt.period_end_timestamp, T0 + INTERVAL);
}

#[test]
fn test_billing_statement_not_found_returns_error() {
    let te = BillingTestEnv::new();
    let result = te.client.try_get_billing_statement(&99, &0);
    assert!(matches!(result, Err(Ok(Error::NotFound))));
}

// ─── Idempotent upsert ────────────────────────────────────────────────────────

#[test]
fn test_billing_statement_upsert_overwrites_same_period() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);

    // First write: period closed, 1 USDC
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);

    // Second write (same period_index): cancellation, updated amounts
    te.client.finalize_billing_statement(
        &te.admin,
        &sub_id,
        &0,
        &subscriber,
        &merchant,
        &T0,
        &(T0 + INTERVAL),
        &PeriodStatementAmounts {
            total_amount_charged: 2_000_000,
            total_usage_units: 0,
            protocol_fee_amount: 0,
            net_amount_to_merchant: 2_000_000,
            refund_amount: 0,
        },
        &(STMT_FLAG_INTERVAL_CHARGED | STMT_FLAG_CANCELLED),
        &BillingStatementFinalization::Cancellation,
    );

    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.total_amount_charged, 2_000_000, "overwrite applied");
    assert_eq!(stmt.finalized_by, BillingStatementFinalization::Cancellation);

    // Index must not have duplicate entries
    let page = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(page.len(), 1, "no duplicate index entries after overwrite");
}

// ─── Subscription pagination ─────────────────────────────────────────────────

#[test]
fn test_bill_stmts_by_sub_pagination() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    for p in 0u32..5 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, (p as i128 + 1) * 1_000_000, BillingStatementFinalization::PeriodClosed);
    }

    // Full page
    let all = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(all.len(), 5);
    assert_eq!(all.get(0).unwrap().period_index, 0);
    assert_eq!(all.get(4).unwrap().period_index, 4);

    // First two
    let p1 = te.client.get_bill_stmts_by_sub(&sub_id, &0, &2);
    assert_eq!(p1.len(), 2);
    assert_eq!(p1.get(0).unwrap().period_index, 0);
    assert_eq!(p1.get(1).unwrap().period_index, 1);

    // Offset into the middle
    let p2 = te.client.get_bill_stmts_by_sub(&sub_id, &3, &2);
    assert_eq!(p2.len(), 2);
    assert_eq!(p2.get(0).unwrap().period_index, 3);
    assert_eq!(p2.get(1).unwrap().period_index, 4);
}

#[test]
fn test_bill_stmts_by_sub_empty() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    let page = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(page.len(), 0, "no statements yet");
}

#[test]
fn test_bill_stmts_by_sub_start_beyond_range_returns_empty() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);

    let page = te.client.get_bill_stmts_by_sub(&sub_id, &100, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn test_bill_stmts_by_sub_limit_zero_returns_empty() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);

    let page = te.client.get_bill_stmts_by_sub(&sub_id, &0, &0);
    assert_eq!(page.len(), 0);
}

// ─── Merchant time-range ──────────────────────────────────────────────────────

#[test]
fn test_bill_stmts_by_merch_rng_basic_filter() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    // Three periods at T0+1*I, T0+2*I, T0+3*I
    for p in 0u32..3 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }

    // Query only the middle period (period_end = T0 + 2*INTERVAL)
    let mid_end = T0 + 2 * INTERVAL;
    let results = te.client.get_bill_stmts_by_merch_rng(
        &merchant, &mid_end, &mid_end, &0, &10,
    );
    assert_eq!(results.len(), 1, "only period 1 matches");
    assert_eq!(results.get(0).unwrap().period_index, 1);
}

#[test]
fn test_bill_stmts_by_merch_rng_all_periods() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    for p in 0u32..4 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }

    let all = te.client.get_bill_stmts_by_merch_rng(&merchant, &0, &u64::MAX, &0, &10);
    assert_eq!(all.len(), 4);
}

#[test]
fn test_bill_stmts_by_merch_rng_no_match_returns_empty() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);

    // Query far in the future — no match
    let future = T0 + 100 * INTERVAL;
    let results = te.client.get_bill_stmts_by_merch_rng(
        &merchant, &future, &(future + INTERVAL), &0, &10,
    );
    assert_eq!(results.len(), 0);
}

#[test]
fn test_bill_stmts_by_merch_rng_pagination_within_range() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    for p in 0u32..6 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }

    let page1 = te.client.get_bill_stmts_by_merch_rng(&merchant, &0, &u64::MAX, &0, &3);
    let page2 = te.client.get_bill_stmts_by_merch_rng(&merchant, &0, &u64::MAX, &3, &3);
    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 3);
    assert_ne!(
        page1.get(0).unwrap().period_index,
        page2.get(0).unwrap().period_index,
        "pages are disjoint"
    );
}

// ─── Index isolation ─────────────────────────────────────────────────────────

#[test]
fn test_billing_statements_isolated_by_subscription() {
    let te = BillingTestEnv::new();
    let merchant = Address::generate(&te.env);
    let sub_a = Address::generate(&te.env);
    let sub_b = Address::generate(&te.env);
    let id_a = te.make_subscription(&sub_a, &merchant);
    let id_b = te.make_subscription(&sub_b, &merchant);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(id_a, 0, &sub_a, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    te.finalize(id_a, 1, &sub_a, &merchant, 2_000_000, BillingStatementFinalization::PeriodClosed);
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(id_b, 0, &sub_b, &merchant, 3_000_000, BillingStatementFinalization::Cancellation);

    let a_stmts = te.client.get_bill_stmts_by_sub(&id_a, &0, &10);
    assert_eq!(a_stmts.len(), 2, "subscription A has 2 periods");

    let b_stmts = te.client.get_bill_stmts_by_sub(&id_b, &0, &10);
    assert_eq!(b_stmts.len(), 1, "subscription B has 1 period");
    assert_eq!(b_stmts.get(0).unwrap().total_amount_charged, 3_000_000);
}

#[test]
fn test_billing_statements_isolated_by_merchant() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant_a = Address::generate(&te.env);
    let merchant_b = Address::generate(&te.env);
    let id_a = te.make_subscription(&subscriber, &merchant_a);
    let id_b = te.make_subscription(&subscriber, &merchant_b);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(id_a, 0, &subscriber, &merchant_a, 1_000_000, BillingStatementFinalization::PeriodClosed);
    te.finalize(id_b, 0, &subscriber, &merchant_b, 2_000_000, BillingStatementFinalization::PeriodClosed);

    let a_range = te.client.get_bill_stmts_by_merch_rng(&merchant_a, &0, &u64::MAX, &0, &10);
    assert_eq!(a_range.len(), 1);
    assert_eq!(a_range.get(0).unwrap().total_amount_charged, 1_000_000);

    let b_range = te.client.get_bill_stmts_by_merch_rng(&merchant_b, &0, &u64::MAX, &0, &10);
    assert_eq!(b_range.len(), 1);
    assert_eq!(b_range.get(0).unwrap().total_amount_charged, 2_000_000);
}

// ─── Security ────────────────────────────────────────────────────────────────

#[test]
fn test_finalize_billing_statement_requires_admin() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);
    let stranger = Address::generate(&te.env);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    let result = te.client.try_finalize_billing_statement(
        &stranger, &sub_id, &0, &subscriber, &merchant,
        &T0, &(T0 + INTERVAL),
        &PeriodStatementAmounts {
            total_amount_charged: 1_000_000,
            total_usage_units: 0,
            protocol_fee_amount: 0,
            net_amount_to_merchant: 1_000_000,
            refund_amount: 0,
        },
        &STMT_FLAG_INTERVAL_CHARGED,
        &BillingStatementFinalization::PeriodClosed,
    );
    assert!(matches!(result, Err(Ok(Error::Unauthorized))), "stranger must be rejected");
}

// ─── Finalization modes ───────────────────────────────────────────────────────

#[test]
fn test_billing_statement_cancellation_mode() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::Cancellation);

    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.finalized_by, BillingStatementFinalization::Cancellation);
}

#[test]
fn test_billing_statement_final_settlement_mode() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.client.finalize_billing_statement(
        &te.admin, &sub_id, &0, &subscriber, &merchant,
        &T0, &(T0 + INTERVAL),
        &PeriodStatementAmounts {
            total_amount_charged: 0,
            total_usage_units: 0,
            protocol_fee_amount: 0,
            net_amount_to_merchant: 0,
            refund_amount: 500_000,
        },
        &(STMT_FLAG_SETTLED | STMT_FLAG_CANCELLED),
        &BillingStatementFinalization::FinalSettlement,
    );

    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.finalized_by, BillingStatementFinalization::FinalSettlement);
    assert_eq!(stmt.refund_amount, 500_000);
    assert_ne!(stmt.status_flags & STMT_FLAG_SETTLED, 0);
    assert_ne!(stmt.status_flags & STMT_FLAG_CANCELLED, 0);
}

// ─── Status flags ─────────────────────────────────────────────────────────────

#[test]
fn test_status_flags_round_trip() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    let flags = STMT_FLAG_INTERVAL_CHARGED | STMT_FLAG_USAGE_CHARGED | STMT_FLAG_ONEOFF_CHARGED;
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    te.client.finalize_billing_statement(
        &te.admin, &sub_id, &0, &subscriber, &merchant,
        &T0, &(T0 + INTERVAL),
        &PeriodStatementAmounts {
            total_amount_charged: 3_000_000,
            total_usage_units: 100,
            protocol_fee_amount: 0,
            net_amount_to_merchant: 3_000_000,
            refund_amount: 0,
        },
        &flags,
        &BillingStatementFinalization::PeriodClosed,
    );

    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.status_flags, flags);
    assert_ne!(stmt.status_flags & STMT_FLAG_INTERVAL_CHARGED, 0);
    assert_ne!(stmt.status_flags & STMT_FLAG_USAGE_CHARGED, 0);
    assert_ne!(stmt.status_flags & STMT_FLAG_ONEOFF_CHARGED, 0);
    assert_eq!(stmt.status_flags & STMT_FLAG_CANCELLED, 0);
    assert_eq!(stmt.total_usage_units, 100);
}

// ─── Coexistence with per-charge audit log ────────────────────────────────────

#[test]
fn test_period_statements_coexist_with_charge_audit_log() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    te.mint(&subscriber, &100_000_000);
    let sub_id = te.make_subscription(&subscriber, &merchant);
    te.client.deposit_funds(&sub_id, &subscriber, &10_000_000);

    // Trigger a real charge → writes a per-charge BillingStatement row
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    te.client.charge_subscription(&sub_id);

    // Write a period statement for the same subscription
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 2);
    te.finalize(sub_id, 0, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);

    // Per-charge audit trail is intact
    let charge_rows = te.client.get_sub_statements_offset(&sub_id, &0, &10, &false);
    assert_eq!(charge_rows.statements.len(), 1, "one per-charge row");
    assert_eq!(charge_rows.statements.get(0).unwrap().amount, 1_000_000);

    // Period statement is also retrievable independently
    let period_stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(period_stmt.total_amount_charged, 1_000_000);
}

// ─── Compaction does not affect period statements ─────────────────────────────

#[test]
fn test_compaction_does_not_remove_period_statements() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    te.mint(&subscriber, &100_000_000);
    let sub_id = te.make_subscription(&subscriber, &merchant);
    te.client.deposit_funds(&sub_id, &subscriber, &50_000_000);

    // Produce 5 per-charge rows
    for i in 1u64..=5 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + i * INTERVAL + 1);
        te.client.charge_subscription(&sub_id);
    }

    // Write period statements
    for p in 0u32..5 {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL + 2);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }

    // Compact per-charge rows down to 2
    te.client.compact_billing_statements(&te.admin, &sub_id, &Some(2u32));

    // Per-charge rows: 2 kept
    let charge_page = te.client.get_sub_statements_offset(&sub_id, &0, &10, &false);
    assert_eq!(charge_page.statements.len(), 2);

    // Period statements: all 5 still present
    let period_page = te.client.get_bill_stmts_by_sub(&sub_id, &0, &10);
    assert_eq!(period_page.len(), 5, "compaction does not touch period statements");
}

// ─── High-volume walk ─────────────────────────────────────────────────────────

#[test]
fn test_period_statement_high_volume_full_walk() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    let total_periods = 20u32;
    for p in 0..total_periods {
        te.env.ledger().with_mut(|l| l.timestamp = T0 + (p as u64 + 1) * INTERVAL);
        te.finalize(sub_id, p, &subscriber, &merchant, 1_000_000, BillingStatementFinalization::PeriodClosed);
    }

    // Walk in pages of 7
    let page_size = 7u32;
    let mut cursor = 0u32;
    let mut seen = std::vec::Vec::new();
    loop {
        let page = te.client.get_bill_stmts_by_sub(&sub_id, &cursor, &page_size);
        if page.len() == 0 {
            break;
        }
        for i in 0..page.len() {
            seen.push(page.get(i).unwrap().period_index);
        }
        cursor += page.len();
    }

    assert_eq!(seen.len(), total_periods as usize, "all {} periods recovered", total_periods);
    for expected in 0..total_periods {
        assert!(seen.contains(&expected), "period {} present", expected);
    }
}

// ─── Accounting invariants ────────────────────────────────────────────────────

#[test]
fn test_period_statement_amounts_match_charge_audit_row() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);

    te.mint(&subscriber, &100_000_000);
    let sub_id = te.make_subscription(&subscriber, &merchant);
    te.client.deposit_funds(&sub_id, &subscriber, &20_000_000);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    te.client.charge_subscription(&sub_id);

    let charge_row = te.client.get_sub_statements_offset(&sub_id, &0, &1, &false);
    let charge_amount = charge_row.statements.get(0).unwrap().amount;

    // Period statement records the same amount
    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 2);
    te.finalize(sub_id, 0, &subscriber, &merchant, charge_amount, BillingStatementFinalization::PeriodClosed);

    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.total_amount_charged, charge_amount, "period amount matches charge row");
    assert_eq!(stmt.net_amount_to_merchant, charge_amount, "net matches (no fee)");
}

// ─── fee amounts round-trip ───────────────────────────────────────────────────

#[test]
fn test_period_statement_fee_amounts_preserved() {
    let te = BillingTestEnv::new();
    let subscriber = Address::generate(&te.env);
    let merchant = Address::generate(&te.env);
    let sub_id = te.make_subscription(&subscriber, &merchant);

    te.env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    let total_charged = 10_000_000i128;
    let fee = 500_000i128;
    te.client.finalize_billing_statement(
        &te.admin, &sub_id, &0, &subscriber, &merchant,
        &T0, &(T0 + INTERVAL),
        &PeriodStatementAmounts {
            total_amount_charged: total_charged,
            total_usage_units: 0,
            protocol_fee_amount: fee,
            net_amount_to_merchant: total_charged - fee,
            refund_amount: 0,
        },
        &STMT_FLAG_INTERVAL_CHARGED,
        &BillingStatementFinalization::PeriodClosed,
    );

    let stmt = te.client.get_billing_statement(&sub_id, &0);
    assert_eq!(stmt.total_amount_charged, total_charged);
    assert_eq!(stmt.protocol_fee_amount, fee);
    assert_eq!(stmt.net_amount_to_merchant, total_charged - fee);
}
