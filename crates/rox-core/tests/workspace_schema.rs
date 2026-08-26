//! Holds the committed workspace schema to the bundle types (ADR 22) and
//! checks the write shape actually validates against it, the way a stock
//! editor with the `$schema` reference resolved would.

use rox_core::settings::{workspace_schema, NamedLayout, WorkspaceBundle};

const COMMITTED: &str = include_str!("../../rox/assets/workspace.schema.json");

fn generated() -> String {
    serde_json::to_string_pretty(&workspace_schema()).expect("schema serializes") + "\n"
}

#[test]
fn committed_schema_matches_the_types() {
    assert_eq!(
        COMMITTED,
        generated(),
        "crates/rox/assets/workspace.schema.json is stale; regenerate it with\n\
         cargo test -p rox-core --test workspace_schema -- --ignored regenerate"
    );
}

#[test]
fn saved_shape_validates_and_required_fields_bite() {
    let validator = jsonschema::validator_for(&workspace_schema()).expect("schema compiles");

    // A bundle as the writer produces it, `$schema` stamp included, with
    // enough optional shape filled in to reach the referenced types.
    let mut bundle = WorkspaceBundle {
        name: "Test".into(),
        ..WorkspaceBundle::default()
    };
    bundle.layouts.push(NamedLayout {
        name: "Main".into(),
        dump: serde_json::json!({ "panel_name": "queue" }),
        size: None,
    });
    bundle.signals.push(rox_viz::signal::Signal::default());
    let mut file = serde_json::to_value(&bundle).expect("bundle serializes");
    file.as_object_mut()
        .expect("a bundle is an object")
        .insert("$schema".into(), "../schemas/workspace.schema.json".into());
    let errors: Vec<String> = validator
        .iter_errors(&file)
        .map(|err| err.to_string())
        .collect();
    assert!(errors.is_empty(), "saved shape rejected: {errors:?}");

    // Deleting a required field is exactly what the editor flags.
    file.as_object_mut().unwrap().remove("version");
    assert!(
        !validator.is_valid(&file),
        "a file without its version passed the schema"
    );
}

/// Not a test: rewrites the committed schema from the types. Run it on
/// purpose after a bundle shape change:
/// `cargo test -p rox-core --test workspace_schema -- --ignored regenerate`
#[test]
#[ignore]
fn regenerate() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rox/assets/workspace.schema.json"
    );
    std::fs::write(path, generated()).expect("write the schema");
}
