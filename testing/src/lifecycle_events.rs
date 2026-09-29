//! Reusable lifecycle-event test harness (Issue #144).
//!
//! Contract tests use this to assert the **content and order** of the
//! canonical events emitted by `shared::lifecycle_events`. A missing, extra,
//! or reordered transition fails the test with the diverging index, so the
//! event contract cannot drift silently.
//!
//! ```ignore
//! use testing::lifecycle_events::*;
//!
//! let expected = [aid_transition_event(&env, 1, LifecycleTransition::Created, STATE_NONE, PENDING)];
//! // ... invoke the contract ...
//! assert_emitted_lifecycle_sequence(&env, &expected);
//! ```

use soroban_sdk::testutils::Events;
use soroban_sdk::{Env, FromVal, Symbol, TryFromVal, Vec};

use shared::lifecycle_events::{LifecycleEvent, LIFECYCLE_TOPIC};

/// Decode every canonical lifecycle payload recorded on the environment.
///
/// Events from other modules (legacy `AID_CREATED`, telemetry, ...) are
/// ignored; only events whose first topic is [`LIFECYCLE_TOPIC`] are returned,
/// in emission order.
pub fn decode_lifecycle_events(env: &Env) -> Vec<LifecycleEvent> {
    let all = env.events().all();
    let mut decoded = Vec::new(env);
    for (_contract, topics, data) in all.iter() {
        if topics.is_empty() {
            continue;
        }
        let prefix: Symbol = Symbol::from_val(env, &topics.get(0).unwrap());
        if prefix == LIFECYCLE_TOPIC {
            decoded.push_back(LifecycleEvent::try_from_val(env, &data).unwrap());
        }
    }
    decoded
}

/// Number of canonical lifecycle events emitted so far.
pub fn lifecycle_event_count(env: &Env) -> u32 {
    decode_lifecycle_events(env).len()
}

/// Assert the environment emitted exactly `expected`, in order.
///
/// Panics if any expected event is missing, if an unexpected lifecycle event
/// is present, or if the order differs.
pub fn assert_emitted_lifecycle_sequence(env: &Env, expected: &[LifecycleEvent]) {
    let observed = decode_lifecycle_events(env);
    assert!(
        observed.len() as usize == expected.len(),
        "lifecycle event count mismatch: expected {}, observed {} (events missing or extra)",
        expected.len(),
        observed.len()
    );

    let mut index = 0usize;
    while index < expected.len() {
        let actual = observed.get(index as u32).unwrap();
        assert!(
            actual == expected[index],
            "lifecycle event mismatch at index {}: expected {:?}, observed {:?} (missing, extra, or reordered)",
            index,
            expected[index],
            actual
        );
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::lifecycle_events::{
        emit_aid_event, emit_proposal_event, LifecycleResource, LifecycleTransition,
        LIFECYCLE_EVENT_SCHEMA_VERSION, STATE_NONE,
    };
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{contract, contractimpl, symbol_short, Address};

    #[contract]
    struct LifecycleHarnessFixture;

    #[contractimpl]
    impl LifecycleHarnessFixture {
        pub fn noop(_env: Env) {}
    }

    fn aid_event(
        id: u64,
        transition: LifecycleTransition,
        from: Symbol,
        to: Symbol,
        actor: &Address,
        timestamp: u64,
    ) -> LifecycleEvent {
        LifecycleEvent {
            schema_version: LIFECYCLE_EVENT_SCHEMA_VERSION,
            resource: LifecycleResource::Aid.as_symbol(),
            resource_id: id,
            transition: transition.as_symbol(),
            from_state: from,
            to_state: to,
            actor: actor.clone(),
            timestamp,
        }
    }

    #[test]
    fn harness_accepts_exact_ordered_sequence() {
        let env = Env::default();
        let actor = Address::generate(&env);
        let contract_id = env.register_contract(None, LifecycleHarnessFixture);

        env.as_contract(&contract_id, || {
            emit_aid_event(&env, 1, LifecycleTransition::Created, STATE_NONE, symbol_short!("pending"), &actor, 1);
            emit_aid_event(&env, 1, LifecycleTransition::Claimed, symbol_short!("pending"), symbol_short!("settled"), &actor, 2);
        });

        let expected = [
            aid_event(1, LifecycleTransition::Created, STATE_NONE, symbol_short!("pending"), &actor, 1),
            aid_event(1, LifecycleTransition::Claimed, symbol_short!("pending"), symbol_short!("settled"), &actor, 2),
        ];
        assert_emitted_lifecycle_sequence(&env, &expected);
        assert_eq!(lifecycle_event_count(&env), 2);
    }

    #[test]
    fn harness_ignores_non_lifecycle_events() {
        let env = Env::default();
        let actor = Address::generate(&env);
        let contract_id = env.register_contract(None, LifecycleHarnessFixture);

        env.as_contract(&contract_id, || {
            // A legacy-style event that must not be counted.
            env.events().publish((symbol_short!("aid"), symbol_short!("created")), 1u64);
            emit_aid_event(&env, 5, LifecycleTransition::Created, STATE_NONE, symbol_short!("pending"), &actor, 1);
        });

        assert_eq!(lifecycle_event_count(&env), 1);
        let decoded = decode_lifecycle_events(&env);
        assert_eq!(decoded.get(0).unwrap().resource_id, 5);
    }

    #[test]
    #[should_panic(expected = "events missing or extra")]
    fn harness_fails_when_a_transition_is_missing() {
        let env = Env::default();
        let actor = Address::generate(&env);
        let contract_id = env.register_contract(None, LifecycleHarnessFixture);

        env.as_contract(&contract_id, || {
            emit_proposal_event(&env, 9, LifecycleTransition::Created, STATE_NONE, symbol_short!("pending"), &actor, 1);
        });

        let expected = [
            LifecycleEvent {
                schema_version: LIFECYCLE_EVENT_SCHEMA_VERSION,
                resource: LifecycleResource::Proposal.as_symbol(),
                resource_id: 9,
                transition: LifecycleTransition::Created.as_symbol(),
                from_state: STATE_NONE,
                to_state: symbol_short!("pending"),
                actor: actor.clone(),
                timestamp: 1,
            },
            LifecycleEvent {
                schema_version: LIFECYCLE_EVENT_SCHEMA_VERSION,
                resource: LifecycleResource::Proposal.as_symbol(),
                resource_id: 9,
                transition: LifecycleTransition::Executed.as_symbol(),
                from_state: symbol_short!("pending"),
                to_state: symbol_short!("executed"),
                actor: actor.clone(),
                timestamp: 2,
            },
        ];
        assert_emitted_lifecycle_sequence(&env, &expected);
    }
}
