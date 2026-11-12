#![allow(dead_code)]

use std::sync::Arc;

use x100_core::time::{FakeClock, Timestamp};

pub fn clock() -> (FakeClock, Arc<FakeClock>) {
    let c = FakeClock::new(Timestamp::from_millis(1_000));
    (c.clone(), Arc::new(c))
}
