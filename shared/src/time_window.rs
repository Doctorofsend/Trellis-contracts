
use soroban_sdk::{contracterror, contracttype, symbol_short, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TimeWindowError {
    TooEarly = 1,
    Expired = 2,
    Stale = 3,
    Invalid = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeWindow {
    pub start: u64,
    pub end: u64,
}

pub fn validate_time_window(env: &Env, window: &TimeWindow) -> Result<(), TimeWindowError> {
    if window.end < window.start {
        return Err(TimeWindowError::Stale);
    }
    let now = env.ledger().timestamp();
    if now < window.start {
        env.events().publish(
            (symbol_short!("timewin"), symbol_short!("early")),
            (now, window.start, window.end),
        );
        return Err(TimeWindowError::TooEarly);
    }
    if now > window.end {
        env.events().publish(
            (symbol_short!("timewin"), symbol_short!("expired")),
            (now, window.start, window.end),
        );
        return Err(TimeWindowError::Expired);
    }
    Ok(())
}
