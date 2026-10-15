//! Uses the protocol-owned validator (npm ci --prefix protocol/schema-tests).
//! This tests exported JSON Schema with an independent validator, not a second
//! implementation of JSON Schema inside the protocol crate.
use std::{path::Path, process::Command};

#[test]
fn exported_schemas_bind_known_discriminators_and_allow_extension_fields() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = Command::new("node")
        .current_dir(&root)
        .arg("protocol/schema-tests/contract.cjs")
        .output()
        .expect("node is required; run npm ci --prefix protocol/schema-tests");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
