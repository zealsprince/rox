//! Holds the committed workspace schema to the bundle types (ADR 22) and
//! checks the write shape validates against it.

use rox_core::settings::{Asset, AssetKind, NamedLayout, WorkspaceBundle, workspace_schema};

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

    // Enough optional shape filled in to reach the referenced types.
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
    bundle.assets.push(Asset {
        id: "plate".into(),
        file: "plate.png".into(),
        kind: AssetKind::Image,
        sha256: "ab".repeat(32),
        bytes: 10,
    });
    let mut file = serde_json::to_value(&bundle).expect("bundle serializes");
    file.as_object_mut()
        .expect("a bundle is an object")
        .insert("$schema".into(), "../schemas/workspace.schema.json".into());
    let errors: Vec<String> = validator
        .iter_errors(&file)
        .map(|err| err.to_string())
        .collect();
    assert!(errors.is_empty(), "saved shape rejected: {errors:?}");

    file.as_object_mut().unwrap().remove("version");
    assert!(
        !validator.is_valid(&file),
        "a file without its version passed the schema"
    );
}

/// A shipped look written by a generator rather than exported from the app
/// still has to be a file the schema accepts.
#[test]
fn every_shipped_workspace_validates() {
    let validator = jsonschema::validator_for(&workspace_schema()).expect("schema compiles");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../rox/assets/workspaces");

    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("the shipped folder reads") {
        let path = entry.expect("an entry reads").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        let text = std::fs::read_to_string(&path).expect("a shipped file reads");
        let file: serde_json::Value = serde_json::from_str(&text).expect("a shipped file parses");
        let errors: Vec<String> = validator
            .iter_errors(&file)
            .map(|err| err.to_string())
            .collect();
        assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        checked += 1;
    }

    assert!(checked > 0);
}

/// Not a test: rewrites the committed schema after a bundle shape change.
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
