//! Validates CI workflow guardrails that prevent stale/duplicated runs.

use std::path::Path;

fn ci_workflow_content() -> Option<String> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let path = workspace_root.join(".github/workflows/fast-free-testing.yml");
    std::fs::read_to_string(&path).ok()
}

#[test]
fn ci_workflow_has_concurrency_control() {
    let content = match ci_workflow_content() {
        Some(c) => c,
        None => {
            println!("Skipping GitHub workflow test in sovereign Propel-only environment");
            return;
        }
    };
    assert!(
        content.contains("concurrency:"),
        "fast-free-testing workflow should define concurrency control"
    );
    assert!(
        content.contains("cancel-in-progress: true"),
        "fast-free-testing workflow should cancel superseded runs"
    );
}
