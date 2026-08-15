//! R1 (schema) parity gate: deserializes the Python side's golden fixture
//! (`tests/fixtures/projects.golden.json`) into `swab::schema::Radar`, reserializes it,
//! and asserts the round-trip is value-identical (order-insensitive) to the original —
//! a free external oracle for the wire contract, no Python process needed. Exits non-zero
//! and prints a diff-relevant message on any mismatch or parse failure.

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let golden_path = std::path::Path::new(manifest_dir)
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("projects.golden.json");
    let text = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("failed to read {golden_path:?}: {e}"));

    let radar: swab::schema::Radar =
        serde_json::from_str(&text).expect("golden fixture failed to deserialize into Radar");
    let reserialized = serde_json::to_string(&radar).expect("Radar failed to reserialize");

    let orig: serde_json::Value = serde_json::from_str(&text).unwrap();
    let round: serde_json::Value = serde_json::from_str(&reserialized).unwrap();

    assert_eq!(orig, round, "golden round-trip mismatch (see values above)");
    println!("golden round-trip OK");
}
