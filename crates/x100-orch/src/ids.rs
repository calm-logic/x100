use x100_core::dice::Dice;
use x100_core::id::Ulid;
use x100_core::time::Clock;

/// A fresh id of any kind: the clock's time and randomness, which core leaves to its callers.
pub fn new_id<T>(clock: &dyn Clock, make: fn(Ulid) -> T) -> T {
    make(Ulid::from_parts(
        clock.now().as_millis(),
        ulid::Ulid::generate().random(),
    ))
}

/// The operating system's randomness, for a daemon whose choices need not repeat.
pub struct OsDice;

impl Dice for OsDice {
    fn roll(&self) -> u64 {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the system's random number generator");
        u64::from_le_bytes(bytes)
    }
}
