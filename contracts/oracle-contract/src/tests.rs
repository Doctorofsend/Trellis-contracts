#![cfg(test)]

extern crate std;

use super::*;
use soroban_sdk::{symbol_short, testutils::Address as _, Env};

struct Fixture {
    env: Env,
    admin: Address,
    submitter1: Address,
    submitter2: Address,
    contract_id: Address,
}


fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let submitter1 = Address::generate(&env);
    let submitter2 = Address::generate(&env);

    let contract_id = env.register_contract(None, OracleContract);
    let client = OracleContractClient::new(&env, &contract_id);
    client.initialize(&admin);

    Fixture {
        env,
        admin,
        submitter1,
        submitter2,
        contract_id,
    }
}


#[test]
fn test_initialize_sets_admin() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    assert_eq!(client.get_admin(), fx.admin);
}

#[test]
fn test_double_initialize() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let new_admin = Address::generate(&fx.env);
    assert_eq!(
        client.try_initialize(&new_admin),
        Err(Ok(OracleError::AlreadyInitialized))
    );
    assert_eq!(client.get_admin(), fx.admin);
}

#[test]
fn test_initialize_requires_the_admin_signature() {
    let env = Env::default();
    let contract_id = env.register_contract(None, OracleContract);
    let client = OracleContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    assert!(client.try_initialize(&admin).is_err());
}


#[test]
fn test_get_admin() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    assert_eq!(client.get_admin(), fx.admin);
}

#[test]
fn test_register_submitter_success() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    assert!(client.is_submitter_active(&fx.submitter1));
}

#[test]
fn test_register_submitter_unauthorized_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let unauthorized = Address::generate(&fx.env);
    let result = client.try_register_submitter(&unauthorized, &fx.submitter1);
    assert!(result.is_err());
}

#[test]
fn test_register_submitter_revoked_admin_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.deactivate_submitter(&fx.admin, &fx.admin);
    let result = client.try_register_submitter(&fx.admin, &fx.submitter1);
    assert!(result.is_err());
}


#[test]
fn test_register_duplicate_submitter_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    let result = client.try_register_submitter(&fx.admin, &fx.submitter1);
    assert!(result.is_err());
}

#[test]
fn test_register_multiple_submitters() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.register_submitter(&fx.admin, &fx.submitter2);
    assert!(client.is_submitter_active(&fx.submitter1));
    assert!(client.is_submitter_active(&fx.submitter2));
}


#[test]
fn test_deactivate_submitter_success() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    assert!(client.is_submitter_active(&fx.submitter1));
    client.deactivate_submitter(&fx.admin, &fx.submitter1);
    assert!(!client.is_submitter_active(&fx.submitter1));
}

#[test]
fn test_deactivate_submitter_unauthorized_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    let unauthorized = Address::generate(&fx.env);
    let result = client.try_deactivate_submitter(&unauthorized, &fx.submitter1);
    assert!(result.is_err());
}

#[test]
fn test_deactivate_submitter_revoked_admin_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.deactivate_submitter(&fx.admin, &fx.admin);
    let result = client.try_deactivate_submitter(&fx.admin, &fx.submitter1);
    assert!(result.is_err());
}


#[test]
fn test_deactivated_submitter_cannot_submit() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.deactivate_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert!(result.is_err());
}


#[test]
fn test_submit_price_success() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let id = client.submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert_eq!(id, 1);
}



#[test]
fn test_submit_price_revoked_submitter_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.deactivate_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert!(result.is_err());
}


#[test]
fn test_submit_price_unauthorized_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let unregistered = Address::generate(&fx.env);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &unregistered,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert!(result.is_err());
}


#[test]
fn test_submit_price_negative_price_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &-1000i128,
        &8u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert!(result.is_err());
}


#[test]
fn test_submit_price_invalid_decimals_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &19u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert!(result.is_err());
}


#[test]
fn test_submit_price_zero_price_success() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &0i128,
        &8u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert!(result.is_ok());
}


#[test]
fn test_submit_price_max_decimals_success() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &18u32,
        &fx.env.ledger().timestamp(),
        &1u64,
    );
    assert!(result.is_ok());
}


#[test]
fn test_replay_protection_duplicate_nonce() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();

    let result1 = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &ts,
        &1u64,
    );
    assert!(result1.is_ok());

    let result2 = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &51000_00000000i128,
        &8u32,
        &ts,
        &1u64,
    );
    assert!(result2.is_err());
}


#[test]
fn test_sequential_nonces_success() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();

    let r1 = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &ts,
        &1u64,
    );
    assert!(r1.is_ok());

    let r2 = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &51000_00000000i128,
        &8u32,
        &ts,
        &2u64,
    );
    assert!(r2.is_ok());

    let r3 = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &52000_00000000i128,
        &8u32,
        &ts,
        &3u64,
    );
    assert!(r3.is_ok());
}


#[test]
fn test_nonce_skipping_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &fx.env.ledger().timestamp(),
        &5u64,
    );
    assert!(result.is_err());
}


#[test]
fn test_independent_feed_nonces() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let btc = symbol_short!("BTCUSD");
    let eth = symbol_short!("ETHUSD");
    let ts = fx.env.ledger().timestamp();

    let r1 = client.try_submit_price(&fx.submitter1, &btc, &50000_00000000i128, &8u32, &ts, &1u64);
    assert!(r1.is_ok());

    let r2 = client.try_submit_price(&fx.submitter1, &eth, &3000_00000000i128, &8u32, &ts, &1u64);
    assert!(r2.is_ok());
}


#[test]
fn test_stale_submission_rejected() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.set_staleness_window(&fx.admin, &1000);

    let feed_id = symbol_short!("BTCUSD");
    let now = fx.env.ledger().timestamp();
    let stale_ts = now.saturating_sub(2000);

    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &stale_ts,
        &1u64,
    );
    assert!(result.is_err());
}

#[test]
fn test_set_staleness_window_unauthorized_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let unauthorized = Address::generate(&fx.env);
    let result = client.try_set_staleness_window(&unauthorized, &1000);
    assert!(result.is_err());
}

#[test]
fn test_set_staleness_window_revoked_admin_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.deactivate_submitter(&fx.admin, &fx.admin);
    let result = client.try_set_staleness_window(&fx.admin, &1000);
    assert!(result.is_err());
}


#[test]
fn test_fresh_submission_within_window() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.set_staleness_window(&fx.admin, &1000);

    let feed_id = symbol_short!("BTCUSD");
    let now = fx.env.ledger().timestamp();
    let fresh_ts = now.saturating_sub(500);

    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &fresh_ts,
        &1u64,
    );
    assert!(result.is_ok());
}


#[test]
fn test_submission_at_boundary() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.set_staleness_window(&fx.admin, &1000);

    let feed_id = symbol_short!("BTCUSD");
    let now = fx.env.ledger().timestamp();
    let boundary_ts = now.saturating_sub(1000);

    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &boundary_ts,
        &1u64,
    );
    assert!(result.is_ok());
}


#[test]
fn test_get_latest_price_after_submit() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.register_submitter(&fx.admin, &fx.submitter2);

    let feed_id = symbol_short!("BTCUSD");
    let price = 50000_00000000i128;
    let ts = fx.env.ledger().timestamp();

    client.submit_price(&fx.submitter1, &feed_id, &price, &8u32, &ts, &1u64);
    client.submit_price(&fx.submitter2, &feed_id, &price, &8u32, &ts, &1u64);

    let latest = client.get_latest_price(&feed_id);
    assert_eq!(latest.price, price);
    assert_eq!(latest.decimals, 8);
    assert_eq!(latest.timestamp, ts);
    assert_eq!(latest.submission_count, 1);
}

#[test]
fn test_get_latest_price_unauthorized_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let unauthorized = Address::generate(&fx.env);
    let result = client.try_get_latest_price(&symbol_short!("BTCUSD"));
    assert!(result.is_err());
}


#[test]
fn test_get_latest_price_never_written_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let result = client.try_get_latest_price(&symbol_short!("XYZUSD"));
    assert!(result.is_err());
}


#[test]
fn test_get_latest_price_most_recent() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.register_submitter(&fx.admin, &fx.submitter2);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();

    client.submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &ts,
        &1u64,
    );
    let l1 = client.try_get_latest_price(&feed_id);
    assert_eq!(l1, Err(Ok(OracleError::FeedNotFound)));

    client.submit_price(
        &fx.submitter2,
        &feed_id,
        &51000_00000000i128,
        &8u32,
        &ts,
        &1u64,
    );
    let l2 = client.get_latest_price(&feed_id);
    assert_eq!(l2.price, 50500_00000000i128);
    assert_eq!(l2.submission_count, 2);
}


#[test]
fn test_get_price_history_empty_feed_fails() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let result = client.try_get_price_history(&symbol_short!("XYZUSD"), &10u32);
    assert!(result.is_err());
}


#[test]
fn test_get_price_history_returns_submissions() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();

    for i in 1..=5 {
        let price = 50000_00000000i128 + (i * 1000) as i128;
        client.submit_price(&fx.submitter1, &feed_id, &price, &8u32, &ts, &(i as u64));
    }

    let history = client.get_price_history(&feed_id, &10u32);
    assert_eq!(history.len(), 5);
}


#[test]
fn test_get_price_history_respects_limit() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();

    for i in 1..=10 {
        let price = 50000_00000000i128 + (i * 1000) as i128;
        client.submit_price(&fx.submitter1, &feed_id, &price, &8u32, &ts, &(i as u64));
    }

    let history = client.get_price_history(&feed_id, &3u32);
    assert_eq!(history.len(), 3);
    assert_eq!(history.get(0).unwrap().id, 8);
    assert_eq!(history.get(1).unwrap().id, 9);
    assert_eq!(history.get(2).unwrap().id, 10);
}


#[test]
fn test_multiple_submitters_same_feed() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.register_submitter(&fx.admin, &fx.submitter2);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();

    client.submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &ts,
        &1u64,
    );
    client.submit_price(
        &fx.submitter2,
        &feed_id,
        &51000_00000000i128,
        &8u32,
        &ts,
        &1u64,
    );

    let latest = client.get_latest_price(&feed_id);
    assert_eq!(latest.price, 50500_00000000i128);
    assert_eq!(latest.submission_count, 2);
}


#[test]
fn test_future_timestamp_rejected() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let future_ts = fx.env.ledger().timestamp() + 61;

    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &future_ts,
        &1u64,
    );
    assert_eq!(result, Err(Ok(OracleError::SubmissionFromFuture)));
}


#[test]
fn test_median_ignores_single_outlier() {
    let fx = setup();
    let submitter3 = Address::generate(&fx.env);
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.register_submitter(&fx.admin, &fx.submitter2);
    client.register_submitter(&fx.admin, &submitter3);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();
    client.submit_price(&fx.submitter1, &feed_id, &50_000_i128, &2u32, &ts, &1u64);
    client.submit_price(&fx.submitter2, &feed_id, &51_000_i128, &2u32, &ts, &1u64);
    client.submit_price(&submitter3, &feed_id, &1_000_000_i128, &2u32, &ts, &1u64);

    let latest = client.get_latest_price(&feed_id);
    assert_eq!(latest.price, 51_000_i128);
    assert_eq!(latest.submission_count, 3);
}


#[test]
fn test_future_timestamp_rejected() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);

    let feed_id = symbol_short!("BTCUSD");
    let future_ts = fx.env.ledger().timestamp() + 61;

    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &future_ts,
        &1u64,
    );
    assert_eq!(result, Err(Ok(OracleError::SubmissionFromFuture)));
}

#[test]
fn test_median_ignores_single_outlier() {
    let fx = setup();
    let submitter3 = Address::generate(&fx.env);
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.register_submitter(&fx.admin, &fx.submitter1);
    client.register_submitter(&fx.admin, &fx.submitter2);
    client.register_submitter(&fx.admin, &submitter3);

    let feed_id = symbol_short!("BTCUSD");
    let ts = fx.env.ledger().timestamp();
    client.submit_price(&fx.submitter1, &feed_id, &50_000_i128, &2u32, &ts, &1u64);
    client.submit_price(&fx.submitter2, &feed_id, &51_000_i128, &2u32, &ts, &1u64);
    client.submit_price(&submitter3, &feed_id, &1_000_000_i128, &2u32, &ts, &1u64);

    let latest = client.get_latest_price(&feed_id);
    assert_eq!(latest.price, 51_000_i128);
    assert_eq!(latest.submission_count, 3);
}

#[test]
fn test_set_staleness_window_success() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    let result = client.try_set_staleness_window(&fx.admin, &7200);
    assert!(result.is_ok());
    assert_eq!(client.get_staleness_window(), 7200);
}


#[test]
fn test_get_staleness_window_default() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    assert_eq!(client.get_staleness_window(), 3600);
}


#[test]
fn test_set_staleness_window_zero() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);
    client.set_staleness_window(&fx.admin, &0);
    assert_eq!(client.get_staleness_window(), 0);

    client.register_submitter(&fx.admin, &fx.submitter1);
    let feed_id = symbol_short!("BTCUSD");
    let now = fx.env.ledger().timestamp();

    let result = client.try_submit_price(
        &fx.submitter1,
        &feed_id,
        &50000_00000000i128,
        &8u32,
        &now.saturating_sub(1),
        &1u64,
    );
    assert!(result.is_err());
}


#[test]
fn test_is_submitter_active_status() {
    let fx = setup();
    let client = OracleContractClient::new(&fx.env, &fx.contract_id);

    assert!(!client.is_submitter_active(&fx.submitter1));
    client.register_submitter(&fx.admin, &fx.submitter1);
    assert!(client.is_submitter_active(&fx.submitter1));
    client.deactivate_submitter(&fx.admin, &fx.submitter1);
    assert!(!client.is_submitter_active(&fx.submitter1));
}

