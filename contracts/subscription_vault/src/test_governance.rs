use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env, String};

#[test]
fn test_merchant_config_initialization() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant_a = Address::generate(&env);
    let payout_address = Address::generate(&env);
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");

    // initialize_merchant_config returns MerchantConfig directly (Soroban unwraps Result<T,E> -> T)
    let config = client.initialize_merchant_config(
        &merchant_a,
        &payout_address,
        &500, // 5% fee in bips
        &0x1F, // all operations enabled
        &None,
        &redirect_url,
    );

    assert_eq!(config.fee_bips, 500);
    assert_eq!(config.is_active, true);
    assert_eq!(config.redirect_url, redirect_url);
}

#[test]
fn test_merchant_config_governance_enforced() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant_a = Address::generate(&env);
    let payout_address = Address::generate(&env);
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");

    // Initialize config first
    client.initialize_merchant_config(
        &merchant_a,
        &payout_address,
        &500,
        &0x1F,
        &None,
        &redirect_url,
    );

    // Partial update — update_merchant_config also returns MerchantConfig directly
    let updated = client.update_merchant_config(
        &merchant_a,
        &None,                                                   // payout unchanged
        &Some(1000),                                             // new fee: 10%
        &None,                                                   // ops unchanged
        &None,                                                   // active unchanged
        &None,                                                   // fee_address unchanged
        &Some(String::from_str(&env, "https://new-url.com")),   // new redirect
        &None,                                                   // paused unchanged
    );

    assert_eq!(updated.fee_bips, 1000);
    assert_eq!(updated.redirect_url, String::from_str(&env, "https://new-url.com"));
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_unauthorized_merchant_config_update() {
    let env = Env::default();
    // No mock_all_auths — require_auth() without a signature triggers a host Auth error.
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,
        &0x1F,
        &None,
        &String::from_str(&env, "https://malicious.com"),
    );
}

// === Edge Cases and Boundary Validation ===

#[test]
fn test_fee_bips_at_maximum_boundary() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // fee_bips = 10000 is the maximum allowed (100%)
    let config = client.initialize_merchant_config(
        &merchant,
        &payout,
        &10000,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );

    assert_eq!(config.fee_bips, 10000);
}

#[test]
#[should_panic(expected = "Error(Contract, #1038)")]
fn test_fee_bips_exceeds_maximum() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // fee_bips = 10001 exceeds MAX_FEE_BIPS — must return InvalidFeeBips (#1038)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &10001,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #1041)")]
fn test_operations_without_charge_flag() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // OP_CHARGE is missing — must return MustAllowChargeOperation (#1041)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &(crate::OP_WITHDRAW | crate::OP_REFUND),
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #1040)")]
fn test_operations_with_invalid_bit() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // Bit 0x80 is not a valid operation — must return InvalidOperations (#1040)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &0x80,
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
fn test_get_merchant_config_not_found() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);

    // get_merchant_config returns Option<MerchantConfig> — None when uninitialized
    let config = client.get_merchant_config(&merchant);
    assert!(config.is_none());
}

#[test]
#[should_panic(expected = "Error(Contract, #1042)")]
fn test_update_nonexistent_config() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);

    // Updating before initialize — must return ConfigNotFound (#1042)
    let _ = client.update_merchant_config(
        &merchant,
        &None,
        &Some(500),
        &None,
        &None,
        &None,
        &None,
        &None,
    );
}

#[test]
fn test_partial_update_preserves_other_fields() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    let initial = client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,   // 5%
        &0x0F,  // no OP_AUTO_RENEWAL
        &None,
        &String::from_str(&env, "https://initial.com"),
    );

    assert_eq!(initial.fee_bips, 500);
    assert_eq!(initial.allowed_operations, 0x0F);

    // Update only fee_bips — everything else must be preserved
    let updated = client.update_merchant_config(
        &merchant,
        &None,
        &Some(1000),
        &None,
        &None,
        &None,
        &None,
        &None,
    );

    assert_eq!(updated.fee_bips, 1000);
    assert_eq!(updated.allowed_operations, 0x0F);
    assert_eq!(updated.redirect_url, String::from_str(&env, "https://initial.com"));
}

#[test]
fn test_set_and_get_merchant_config() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    client.initialize_merchant_config(
        &merchant,
        &payout,
        &250,
        &0x1F,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    // get_merchant_config returns Option<MerchantConfig>, so .unwrap() is valid here
    let retrieved = client.get_merchant_config(&merchant).unwrap();

    assert_eq!(retrieved.fee_bips, 250);
    assert_eq!(retrieved.is_active, true);
    assert_eq!(retrieved.allowed_operations, 0x1F);
}

#[test]
fn test_update_deactivate_merchant() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );

    let updated = client.update_merchant_config(
        &merchant,
        &None,
        &None,
        &None,
        &Some(false), // deactivate
        &None,
        &None,
        &None,
    );

    assert_eq!(updated.is_active, false);
}
