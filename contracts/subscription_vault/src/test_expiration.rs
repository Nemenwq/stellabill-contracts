#![cfg(test)]
extern crate std;

use super::*;
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{token, Address, Env};

const T0: u64 = 1_000_000;
const INTERVAL: u64 = 60; // minimum valid interval (MIN_SUBSCRIPTION_INTERVAL_SECONDS)

fn setup_test_env() -> (
    Env,
    SubscriptionVaultClient<'static>,
    token::Client<'static>,
    token::StellarAssetClient<'static>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let token_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_client = token::Client::new(&env, &token_id.address());
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id.address());

    let min_topup = 1_000_000i128;
    client.init(
        &token_id.address(),
        &6,
        &admin,
        &min_topup,
        &(7 * 24 * 60 * 60),
    );

    (env, client, token_client, token_admin_client, admin)
}

// doc 3: charge_subscription rejected at expiry boundary and after;
// withdrawal allowed after expiry (doc 5, Flow 1 steps 1-3, 6)
#[test]
fn test_expiration_timing_and_charging() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    let expires_at = T0 + 2 * INTERVAL;

    token_admin.mint(&subscriber, &(amount * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(expires_at),
    );
    client.deposit_funds(&sub_id, &subscriber, &(amount * 5));

    // Before expiry: charge succeeds
    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL);
    client.charge_subscription(&sub_id);
    assert_eq!(client.get_subscription(&sub_id).lifetime_charged, amount);

    // At expiry boundary (current_time >= expires_at): charge rejected
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    assert!(client.try_charge_subscription(&sub_id).is_err());

    // expires_at field is preserved on the subscription
    assert!(client.get_subscription(&sub_id).expires_at.is_some());

    // After expiry: still rejected
    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    assert!(client.try_charge_subscription(&sub_id).is_err());

    // Withdrawal is allowed after expiry (doc 3 allowed ops, 5)
    let before = token.balance(&subscriber);
    client.withdraw_subscriber_funds(&sub_id, &subscriber);
    assert!(token.balance(&subscriber) > before);
}

// doc 3: charge_usage rejected when expired
#[test]
fn test_usage_charge_rejected_when_expired() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    token_admin.mint(&subscriber, &(amount * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &true, // usage_enabled
        &None::<i128>,
        &Some(T0 + INTERVAL),
    );
    client.deposit_funds(&sub_id, &subscriber, &(amount * 5));

    // Advance past expiry
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    assert!(client.try_charge_usage(&sub_id, &amount).is_err());
}

// doc 3: deposit_funds rejected when expired
#[test]
fn test_deposit_rejected_when_expired() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    token_admin.mint(&subscriber, &(amount * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + INTERVAL),
    );

    // Advance past expiry
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    assert!(client.try_deposit_funds(&sub_id, &subscriber, &amount).is_err());
}

// doc 3: cancel_subscription rejected when expired ("mutually exclusive in behavior")
#[test]
fn test_cancel_rejected_when_expired() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    token_admin.mint(&subscriber, &(amount * 2));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + INTERVAL),
    );

    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    assert!(client.try_cancel_subscription(&sub_id, &subscriber).is_err());
}

// doc 4, 5, Flow 1 (steps 4-6): cleanup before terminal fails; expired -> Archived;
// archived is readable; fund safety after archival
#[test]
fn test_cleanup_and_archival() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    token_admin.mint(&subscriber, &(amount * 5));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + 2 * INTERVAL),
    );
    client.deposit_funds(&sub_id, &subscriber, &(amount * 5));

    // Cleanup before terminal state must fail
    assert!(client.try_cleanup_subscription(&sub_id, &subscriber).is_err());

    // Advance past expiry
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);

    // Cleanup succeeds and transitions to Archived
    client.cleanup_subscription(&sub_id, &subscriber);
    let archived = client.get_subscription(&sub_id);
    assert_eq!(archived.status, SubscriptionStatus::Archived);

    // Archived entity is still readable with correct fields (doc 4 "Readability")
    assert_eq!(archived.amount, amount);
    assert!(archived.expires_at.is_some());

    // Remaining prepaid balance is still withdrawable (doc 5 fund safety)
    if archived.prepaid_balance > 0 {
        let before = token.balance(&subscriber);
        client.withdraw_subscriber_funds(&sub_id, &subscriber);
        assert!(token.balance(&subscriber) > before);
    }
}

// doc 2, 4, Flow 2: cancel before expiry -> Cancelled -> Archived;
// expired path: cancel rejected, cleanup -> Archived
#[test]
fn test_expiration_vs_cancellation() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    token_admin.mint(&subscriber, &(1_000_000i128 * 4));

    // Flow 2: cancel before expiry -> Cancelled -> Archived
    let sub_id1 = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + 3 * INTERVAL),
    );
    client.cancel_subscription(&sub_id1, &subscriber);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Cancelled);

    // Status stays Cancelled even after the would-be expiry time passes
    env.ledger().with_mut(|l| l.timestamp = T0 + 4 * INTERVAL);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Cancelled);

    client.cleanup_subscription(&sub_id1, &subscriber);
    assert_eq!(client.get_subscription(&sub_id1).status, SubscriptionStatus::Archived);

    // Flow 1: expire without cancel -> cancel rejected -> cleanup -> Archived
    let sub_id2 = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &1_000_000i128,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + 2 * INTERVAL),
    );

    env.ledger().with_mut(|l| l.timestamp = T0 + 3 * INTERVAL);
    assert!(client.try_cancel_subscription(&sub_id2, &subscriber).is_err());

    client.cleanup_subscription(&sub_id2, &subscriber);
    assert_eq!(client.get_subscription(&sub_id2).status, SubscriptionStatus::Archived);
}

// doc 1: None expires_at means subscription runs indefinitely
#[test]
fn test_no_expiry_runs_indefinitely() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    token_admin.mint(&subscriber, &(amount * 10));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &None::<u64>,
    );
    client.deposit_funds(&sub_id, &subscriber, &(amount * 10));

    // Far in the future: should still charge successfully
    env.ledger().with_mut(|l| l.timestamp = T0 + 1_000 * INTERVAL);
    client.charge_subscription(&sub_id);
    assert!(client.get_subscription(&sub_id).lifetime_charged > 0);
}

// doc 7: SubscriptionExpiredEvent emitted on first expiry detection;
// SubscriptionArchivedEvent emitted on cleanup
#[test]
fn test_expiration_and_archival_events() {
    let (env, client, token, token_admin, _) = setup_test_env();
    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    let amount = 1_000_000i128;
    token_admin.mint(&subscriber, &(amount * 3));

    let sub_id = client.create_subscription_with_token(
        &subscriber,
        &merchant,
        &token.address,
        &amount,
        &INTERVAL,
        &false,
        &None::<i128>,
        &Some(T0 + INTERVAL),
    );
    client.deposit_funds(&sub_id, &subscriber, &(amount * 3));

    // Trigger expiry detection via charge attempt
    env.ledger().with_mut(|l| l.timestamp = T0 + 2 * INTERVAL);
    let _ = client.try_charge_subscription(&sub_id);

    // SubscriptionExpiredEvent must have been emitted
    let events = env.events().all();
    let expired_event = events.iter().any(|e| {
        let topics: soroban_sdk::Vec<soroban_sdk::Val> = e.0;
        topics.iter().any(|t| {
            if let Ok(sym) = soroban_sdk::Symbol::try_from_val(&env, &t) {
                sym == soroban_sdk::Symbol::new(&env, "subscription_expired")
            } else {
                false
            }
        })
    });
    assert!(expired_event, "SubscriptionExpiredEvent not emitted");

    // Cleanup emits SubscriptionArchivedEvent
    client.cleanup_subscription(&sub_id, &subscriber);

    let events_after = env.events().all();
    let archived_event = events_after.iter().any(|e| {
        let topics: soroban_sdk::Vec<soroban_sdk::Val> = e.0;
        topics.iter().any(|t| {
            if let Ok(sym) = soroban_sdk::Symbol::try_from_val(&env, &t) {
                sym == soroban_sdk::Symbol::new(&env, "subscription_archived")
            } else {
                false
            }
        })
    });
    assert!(archived_event, "SubscriptionArchivedEvent not emitted");
}
