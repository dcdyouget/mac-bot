# JSON Schema contract validation

This directory owns the pinned Ajv validator used by the Rust schema contract test.
It does not depend on either client application. Node.js and npm are required.

From the repository root:

```sh
npm ci --prefix protocol/schema-tests
cargo run --manifest-path protocol/rust/Cargo.toml --bin generate-schema
cargo test --manifest-path protocol/rust/Cargo.toml
```

To run only the schema checks:

```sh
node protocol/schema-tests/contract.cjs
```

Missing validator dependencies fail explicitly; tests are never skipped.
Unknown object fields remain accepted. Known method/event/trace discriminators
must use their corresponding payload, including nested trace items. Unknown
enum names retain the existing schema boundary; clients handle them through
their raw compatibility layers.
