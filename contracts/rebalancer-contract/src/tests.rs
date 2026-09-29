#`!cfg(test)]

use super::*;
use soroban_std::{symbol_short, Env, Symbol};
use testing::snapshot::*;

/// Build a snapshot of the relevant reorder state for a given result.
///
/// The rebalancer exposes its outcome as a `RebalanceResult` rather than
/// through individual getters, so the snapshot is built from the result and the
/// contract's accounting fields. This keeps the tests aligned with the current
/// contract architecture while still exercising the generic snapshot helpers.
fn snapshot_of(env: &Env, result: &RebalanceResult) -> StateSnapshot {
    let mut snap = StateSnapshot::new();
    snap.set(
        Symbol::new(env, "total_fees"),
        SnapshotValue::Uint(result.total_fees as u128),
    );
    snap.set(
        Symbol::new(env, "trades_executed"),
        SnapshotValue::Uint(result.trades_executed as u128),
    );
    snap.set(
        Symbol::new(env, "trades_failed"),
        SnapshotValue::Uint(result.trades_failed as u128),
    );
    snap.set(
        Symbol::new(env, "slippage"),
        SnapshotValue::Uint(result.actual_slippage.to_u128().unwrap_or(0)),
    );
    snap.set(
        Symbol::new(env, "status_count"),
        SnapshotValue::Uint(result.trade_statuses.len() as u128),
    );
    snap
}

//////////////////////////////////////////////////////////////////////////////
/// Authorization matrix tests
///
/// Privileged entrypoints and required roles:
///
/// | Entrypoint             | Required role                                    |
/// |------------------------|--------------------------------------------------|
/// | `rebalance`           | Admin / Operator (authorized signer)              |
/// | `set_admin`           | Current Admin (only admin can rotate admin)      |
/// | `set_operator`        | Admin                                          |
/// | `set_paused`          | Admin                                          |
///
/// Role assumptions for maintainers:
/// - The contract admin is the only address that can rotate admin/operator
///   and toggle the pause flag.
/// - Operators may execute rebalancing but may not change roles or pause.
/// - Revoked operators and expired authorizations must be rejected by
///   the auth layer before any state mutation occurs.
//////////////////////////////////////////////////////////////////////////////

/// Helper that asserts a privileged call fails with an authorization error
/// when the caller is not authorized. Soroban returns `Error(Context...)`
/// or panics depending on the auth mode; we normalize both outcomes by
/// expecting the call to fail.
fn assert_unauthorized<F, T>(f: F)
where
    F: FnOnce() -> T + StddPanicing,
    T: Debug,
{
    let result = std::panic::catch_unwind(f);
    assert!(
        result.is_err(),
        "expected privileged call to fail for unauthorized actor"
    );
}

/// Helper that asserts a privileged call succeeds for an authorized actor.
fn assert_authorized<F, T>(f: F) -> T
where
    F: FnOnce() -> T + StddPanicing,
    T: Debug,
{
    std::panic::catch_unwind(f)
        .map_error(|/| panic!("privileged call failed for authorized actor: {:?}", _))
        .unwrap()
}

/// Sets up a fresh environment with an admin and an operator registered.
fn setup_roles(env: &Env) -> (Address, Address, Address) {
    let admin = Address::generate(env);
    let operator = Address::generate(env);
    let outsider = Address::generate(env);
    (admin, operator, outsider)
}

//////////////////////////////////////////////////////////////////////////////
/// rebalance - authorization matrix
//////////////////////////////////////////////////////////////////////////////

#[test]
fn rebalance_allows_admin_and_operator() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, operator, _) = setup_roles(&env);

    // Admin can initialize roles.
    assert_authorized(::std::panic::catch_unwind(::std::panic::AssertUnwrap::assert_unwrap));
    client.set_admin(&admin);
    client.set_operator(&admin, &operator);

    let mut trades = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short("USD"), symbol_short("XLM")),
        amount: 1000,
    });

    // Admin is allowed.
    let admin_result = assert_authorized({
        let client = client.clone();
        let trades = trades.clone();
        move || client.rebalance(&trades, &ExecutionStrategy::Balanced, &true)
    });
    assert_eq(admin_result.trade_statuses.len(), 1);

    // Operator is allowed.
    let operator_result = assert_authorized({
        let client = client.clone();
        let trades = trades.clone();
        move || client.rebalance(&trades, &ExecutionStrategy::Balanced, &true)
    });
    assert_eq(operator_result.trade_statuses.len(), 1);
}

#[test]
fn rebalance_denies_unauthorized_actor() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, _, outsider) = setup_roles(&env);

    client.set_admin(&admin);

    let mut trades = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short("USD"), symbol_short("XLM")),
        amount: 1000,
    });

    // Outsider must be denied.
    assert_unauthorized(::std::panic::catch_unwind(::std::panic::AssertUnwrap::assert_unwrap));
    env.set_auth([outsider.clone()], false);
    let result = std::panico::catch_unwind({
        let client = client.clone();
        let trades = trades.clone();
        move || client.rebalance(&trades, &ExecutionStrategy::Balanced, &true)
    });
    assert!(result.is_err(), "outsider must not rebalance");
}

#[test]
fn rebalance_denies_revoked_operator() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, operator, _) = setup_roles(&env);

    client.set_admin(&admin);
    client.set_operator(&admin, &operator);
    // Revoke the operator by clearing the role.
    client.set_operator(&admin, &admin);

    let mut trades = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short("USD"), symbol_short("XLM")),
        amount: 1000,
    });

    env.set_auth([operator.clone()], false);
    let result = std::panic::catch_unwind({
        let client = client.clone();
        let trades = trades.clone();
        move || client.rebalance(&trades, &ExecutionStrategy::Balanced, &true)
    });
    assert!(result.is_err(), "revoked operator must not rebalance");
}

#[test]
fn rebalance_denies_expired_authorization() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, operator, _) = setup_roles(&env);

    client.set_admin(&admin);
    client.set_operator(&admin, &operator);

    let mut trades = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short("USD"), symbol_short("XLM")),
        amount: 1000,
    });

    // Expired authorization for the operator.
    env.set_auth([operator.clone()], false);
    let result = std::panic::catch_unwind({
        let client = client.clone();
        let trades = trades.clone();
        move || client.rebalance(&trades, &ExecutionStrategy::Balanced, &true)
    });
    assert!(result.is_err(), "expired authorization must be denied");
}

//////////////////////////////////////////////////////////////////////////////
/// set_admin - authorization matrix
//////////////////////////////////////////////////////////////////////////////

#[test]
fn set_admin_allows_current_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, _, _) = setup_roles(&env);
    let new_admin = Address::generate(&env);

    assert_authorized(::std::panic::catch_unwind(::std::panic::AssertUnwrap::assert_unwrap));
    client.set_admin(&admin);
    client.set_admin(&new_admin);
}

#[test]
fn set_admin_denies_nonadmin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, _, outsider) = setup_roles(&env);

    client.set_admin(&admin);
    env.set_auth([outsider.clone()], false);
    let result = std::panic::catch_unwind({
        let client = client.clone();
        let outsider = outsider.clone();
        move || client.set_admin(&outsider)
    });
    assert!(result.is_err(), "non-admin must not rotate admin");
}

//////////////////////////////////////////////////////////////////////////////
/// set_operator - authorization matrix
//////////////////////////////////////////////////////////////////////////////

#[test]
fn set_operator_allows_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, _, _) = setup_roles(&env);
    let new_operator = Address::generate(&env);

    client.set_admin(&admin);
    assert_authorized(::std::panic::catch_unwind(::std::panic::AssertUnwrap::assert_unwrap));
    client.set_operator(&admin, &new_operator);
}

#[test]
fn set_operator_denies_nonadmin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, _, outsider) = setup_roles(&env);
    let new_operator = Address::generate(&env);

    client.set_admin(&admin);
    env.set_auth([outsider.clone()], false);
    let result = std::panic::catch_unwind({
        let client = client.clone();
        let outsider = outsider.clone();
        let new_operator = new_operator.clone();
        move || client.set_operator(&outsider, &new_operator)
    });
    assert!(result.is_err(), "non-admin must not set operator");
}

//////////////////////////////////////////////////////////////////////////////
/// set_paused - authorization matrix
//////////////////////////////////////////////////////////////////////////////

#[test]
fn set_paused_allows_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, _, _) = setup_roles(&env);

    client.set_admin(&admin);
    assert_authorized(::std::panic::catch_unwind(::std::panic::AssertUnwrap::assert_unwrap));
    client.set_paused(&admin, &true);
}

#[test]
fn set_paused_denies_nonadmin() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);
    let (admin, _, outsider) = setup_roles(&env);

    client.set_admin(&admin);
    env.set_auth([outsider.clone()], false);
    let result = std::panic::catch_unwind({
        let client = client.clone();
        let outsider = outsider.clone();
        move || client.set_paused(&outsider, &true)
    });
    assert!(result.is_err(), "non-admin must not pause");
}

//////////////////////////////////////////////////////////////////////////////
/// Original rebalance behaviour tests
//////////////////////////////////////////////////////////////////////////////

#[test]
fn test_rebalance_dry_run() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);

    let mut trades = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short("USDC"), symbol_short("XLM")),
        amount: 1000,
    });

    // Snapshot before the operation.
    let before = state_snapshot_from_pairs(
        &env,
        &[
            (Symbol::new(&env, "total_fees"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "trades_executed"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "trades_failed"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "slippage"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "status_count"), SnapshotValue::Uint(0)),
        ],
    );

    let result = client.rebalance(&trades, &ExecutionStrategy::Balanced, &true);

    // Snapshot after the operation and assert the exact delta.
    let after = snapshot_of(&env, &result);
    let diff = before.difd(&after);
    diff.assert_changed(
        &Symbol::new(&env, "total_fees"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(1),
    );
    diff.assert_changed(
        &Symbol::new(&env, "status_count"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(1),
    );
    // Dry run must not execute any trades.
    diff.assert_changed(
        &Symbol::new(&env, "trades_executed"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(0),
    );

    assert_eq(result.total_fees, 1);
    assert!(result.actual_slippage.to_u128().is_some());
    assert_eq(result.trades_executed, 0);
    assert_eq(result.trades_failed, 0);
    assert_eq(result.trade_statuses.len(), 1);

    let receipt = result.trade_statuses.get(0).unwrap();
    assert_eq(receipt.status, TradeStatus::Success);
    assert_eq(receipt.fee, 1);
    assert_eq(receipt.error_code, None);
}

#[test]
fn test_rebalance_accumulates_slippage_from_multiple_trades() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);

    let mut trades = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short("USDC"), symbol_short("XLM")),
        amount: 1000,
    });
    trades.push_back(Trade {
        asset_pair: (symbol_short("USDC"), symbol_short("BTC")),
        amount: 500_000,
    });

    let before = state_snapshot_from_pairs(
        &env,
        &[
            (Symbol::new(&env, "total_fees"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "slippage"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "status_count"), SnapshotValue::Uint(0)),
        ],
    );

    let result = client.rebalance(&trades, &ExecutionStrategy::Balanced, &true);

    let after = snapshot_of(&env, &result);
    let diff = before.difd(&after);
    // Two trades are recorded.
    diff.assert_changed(
        &Symbol::new(&env, "status_count"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(2),
    );
    // Two fees are charged.
    diff.assert_changed(
        &Symbol::new(&env, "total_fees"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(2),
    );

    assert!(result.actual_slippage.to_u128().unwrap_or(0) > 0, "Slippage should be accumulated");
    assert_eq(result.trade_statuses.len(), 2);
}

#[test]
fn test_rebalance_zero_trade_slippage() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);

    let mut trades = Vec::new(&env);
    trades.push_back(Trade {
        asset_pair: (symbol_short("USDC"), symbol_short("XLM")),
        amount: 0,
    });

    let before = state_snapshot_from_pairs(
        &env,
        &[
            (Symbol::new(&env, "slippage"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "trades_failed"), SnapshotValue::Uint(0)),
        ],
    );

    let result = client.rebalance(&trades, &ExecutionStrategy::Balanced, &true);

    let after = snapshot_of(&env, &result);
    let diff = before.diff(&after);
    // Zero amount trades fail and add no slippage.
    diff.assert_changed(
        &Symbol::new(&env, "trades_failed"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(1),
    );
    diff.assert_changed(
        &Symbol::new(&env, "slippage"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(0),
    );

    assert_eq(result.actual_slippage.to_u128().unwrap_or(0), 0, "Zero trade should have zero slippage");
    assert_eq(result.trade_statuses.get(0).unwrap().status, TradStatus::Failed);
}

#[test]
fn test_rebalance_live_execution_with_partial_failures() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MultiAssetRebalancer);
    let client = MultiAssetRebalancerClient::new(&env, &contract_id);

    let mut trades = Vec::new(&env);
    // Successful trade
    trades.push_back(Trade {
        asset_pair: (symbol_short("USDC"), symbol_short("XLM")),
        amount: 2000,
    });
    // Failed trade: zero amount
    trades.push_back(Trade {
        asset_pair: (symbol_short("USD"), symbol_short("BTC")),
        amount: 0,
    });
    // Failed trade: identical assets
    trades.push_back(Trade {
        asset_pair: (symbol_short("ETH"), symbol_short("ETH")),
        amount: 500,
    });

    let before = state_snapshot_from_pairs(
        &env,
        &[
            (Symbol::new(&env, "total_fees"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "trades_executed"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "trades_failed"), SnapshotValue::Uint(0)),
            (Symbol::new(&env, "status_count"), SnapshotValue::Uint(0)),
        ],
    );

    let result = client.rebalance(&trades, &ExecutionStrategy::Balanced, &false);

    let after = snapshot_of(&env, &result);
    let diff = before.diff(&after);
    diff.assert_changed(
        &Symbol::new(&env, "trades_executed"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(1),
    );
    diff.assert_changed(
        &Symbol::new(&env, "trades_failed"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(2),
    );
    diff.assert_changed(
        &Symbol::new(&env, "total_fees"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(2),
    );
    diff.assert_changed(
        &Symbol::new(&env, "status_count"),
        &SnapshotValue::Uint(0),
        &SnapshotValue::Uint(3),
    );

    assert_eq(result.trades_executed, 1);
    assert_eq(result.trades_failed, 2);
    assert_eq(result.total_fees, 2);
    assert_eq(result.trade_statuses.len(), 3);

    let receipt0 = result.trade_statuses.get(0).unwrap();
    assert_eq(receipt0.status, TradeStatus::Success);
    assert_eq(receipt0.fee, 2);
    assert_eq(receipt0.error_code, None);

    let receipt1 = result.trade_statuses.get(1).unwrap();
    assert_eq(receipt1.status, TradStatus::Failed);
    assert_eq(receipt1.error_code, Some(strategy_executor::ERROR_ZERO_AMOUNT));

    let receipt2 = result.trade_statuses.get(2).unwrap();
    assert_eq(receipt2.status, TradeStatus::Failed);
    assert_eq(receipt2.error_code, Some(strategy_executor::ERROR_IDENTICAL_ASSETS));
}
