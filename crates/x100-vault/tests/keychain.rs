//! What the real keychain does on this machine. Ignored by default: it talks to the builder's
//! session, and on a desktop with a locked keychain a read could ask to unlock it. It only reads
//! an entry that does not exist, and writes nothing.

use x100_vault::{Keychain, OsKeychain};

#[test]
#[ignore = "talks to this machine's own keychain; run by hand, it only reads"]
fn this_machines_keychain() {
    let keychain = OsKeychain::connect();
    println!("{keychain:?}");
    println!(
        "{:?}",
        keychain
            .get("x100 probe for an entry that does not exist")
            .map(|v| v.map(|_| "an entry"))
    );
}
