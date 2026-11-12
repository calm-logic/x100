use x100_core::id::Ulid;
use x100_core::time::Clock;

/// A fresh id of any kind: the clock's time and randomness, which core leaves to its callers.
pub fn new_id<T>(clock: &dyn Clock, make: fn(Ulid) -> T) -> T {
    make(Ulid::from_parts(
        clock.now().as_millis(),
        ulid::Ulid::generate().random(),
    ))
}
