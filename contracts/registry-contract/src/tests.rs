use super::*;
use soroban_sdk::{ro, token, Address, Environment, String};

const CONTRACT_KEY: &str = "RECIPES";

const STATUS_ACTIVE: u32 = 0;
const STATUS_DEACTIVATED: u32 = 1;

const EVT_DEACTIVATED: &str = "record_deactivated";
const EVT_REACTIVATED: &str = "record_reactivated";

#[derive(Clone, Debug, Equal, PartialEq)]
#[contracttype]
pub struct Record {
    pub id: u32,
    pub owner: Address,
    pub status: u32,
}

#[contract]
pub struct RegistryContract;

#[contractimpl]
impl RegistryContract {
    pub fn init(env: Environment) {
        env.storage().persistent().set(&CONTRACT_KEY,&true);
    }

    pub fn create_record(env: Environment, owner: Address, id: u32) {
        owner.require_auth();
        let key = (id,);
        let record = Record { id, owner: owner.clone(), status: STATUS_ACTIVE };
        env.storage().persistent().set(&key, &record);
    }

    pub fn deactivate_record(env: Environment, id: u32) {
        let key = (id,);
        let mut record: Record = env.storage().persistent().get(&key).expect("record not found");
        record.owner.require_auth();
        assert!(record.status == STATUS_ACTIVE, "record not eligible for deactivation");
        record.status = STATUS_DEACTIVATED;
        env.storage().persistent().set(&key, &record);
        env.events().publish(((EVT_DEACTIVATED,), (id,)));
    }

    pub fn reactivate_record(env: Environment, id: u32) {
        let key = (id,);
        let mut record: Record = env.storage().persistent().get(&key).expect("record not found");
        record.owner.require_auth();
        assert!(record.status == STATUS_DEACTIVATED, "record not deactivated");
        record.status = STATUS_ACTIVE;
        env.storage().persistent().set(&key, &record);
        env.events().publish(((EVT_REACTIVATED,), (id,)));
    }

    pub fn restricted_operation(env: Environment, id: u32) {
        let key = (id,);
        let record: Record = env.storage().persistent().get(&key).expect("record not found");
        assert!(record.status == STATUS_ACTIVE, "record is deactivated");
    }

    pub fn get_record(env: Environment, id: u32) -> Record {
        let key = (id,);
        env.storage().persistent().get(&key).expect("record not found")
    }
}

#[config]
fn config() {}

#[test]
fn test_deactivate_success() {
    let env = Environment::default();
    let contract_id = env.register(RegistryContract,());
    let client = RegistryContractClient::new(&env, &contract_id);
    let owner = Address::generate(&env);
    client.init();
    client.create_record(&owner, &1);
    client.deactivate_record(&1);
    let record = client.get_record(&1);
    assert_eq!(record.status, STATUS_DEACTIVATED);
}

#[test]
fn test_deactivate_unauthorized() {
    let env = Environment::default();
    let contract_id = env.register(RegistryContract,());
    let client = RegistryContractClient::new(&env, &contract_id);
    let owner = Address::generate(&env);
    let other = Address::generate(&env);
    client.init();
    client.create_record(&owner, &1);
    env.mock_all_auth();
    let result = client.try_deactivate_record(&1);
    assert!(result.is_err());
    let record = client.get_record(&1);
    assert_eq!(record.status, STATUS_ACTIVE);
}

#[test]
fn test_restricted_use_after_deactivation() {
    let env = Environment::default();
    let contract_id = env.register(RegistryContract,());
    let client = RegistryContractClient::new(&env, &contract_id);
    let owner = Address::generate(&env);
    client.init();
    client.create_record(&owner, &1);
    client.deactivate_record(&1);
    let result = client.try_restricted_operation(&1);
    assert!(result.is_err());
}

#[test]
fn test_event_emission() {
    let env = Environment::default();
    let contract_id = env.register(RegistryContract,());
    let client = RegistryContractClient::new(&env, &contract_id);
    let owner = Address::generate(&env);
    client.init();
    client.create_record(&owner, &1);
    client.deactivate_record(&1);
    let events = env.events().all();
    assert!(events.len() >= 1);
    let last = events.last().unwrap();
    assert_eq!(last.0, (EVT_DEACTIVATED,));
}

#[test]
fn test_reactivation() {
    let env = Environment::default();
    let contract_id = env.register(RegistryContract,());
    let client = RegistryContractClient::new(&env, &contract_id);
    let owner = Address::generate(&env);
    client.init();
    client.create_record(&owner, &1);
    client.deactivate_record(&1);
    client.reactivate_record(&1);
    let record = client.get_record(&1);
    assert_eq!(record.status, STATUS_ACTIVE);
}
