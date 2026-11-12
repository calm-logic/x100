use x100_core::file::ContentHash;
use x100_store::{Error, FileStore};

#[test]
fn bytes_come_back_under_their_hash_and_the_same_bytes_are_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let files = FileStore::open(dir.path().join("files")).unwrap();
    let a = files.put(b"hello").unwrap();
    let b = files.put(b"hello").unwrap();
    let c = files.put(b"hello!").unwrap();
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(files.read(&a).unwrap(), b"hello");
    assert!(files.contains(&c).unwrap());
    // The hash is of the content, so it is the same on every machine.
    assert_eq!(
        a.0,
        "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn removing_a_file_removes_the_bytes_and_says_whether_there_were_any() {
    let dir = tempfile::tempdir().unwrap();
    let files = FileStore::open(dir.path()).unwrap();
    let h = files.put(&[0u8; 100_000]).unwrap();
    assert!(files.remove(&h).unwrap());
    assert!(!files.remove(&h).unwrap());
    assert!(matches!(files.read(&h), Err(Error::NoSuchFile(_))));
}

#[test]
fn a_hash_that_could_point_out_of_the_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let files = FileStore::open(dir.path().join("files")).unwrap();
    for bad in ["../../etc/passwd", "sha256:../..", "abc", "sha256:", ""] {
        assert!(matches!(
            files.read(&ContentHash(bad.into())),
            Err(Error::BadHash(_))
        ));
    }
}
