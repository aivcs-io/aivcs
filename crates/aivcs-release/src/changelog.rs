//! CHANGELOG section extraction for GitHub release notes.

use anyhow::{bail, Result};

/// Extract the body under `## [version]` until the next `## [` heading.
pub fn extract_section(changelog: &str, version: &str) -> Result<String> {
    let header = format!("## [{version}]");
    let mut lines = changelog.lines().peekable();
    let mut found = false;
    let mut out = String::new();

    while let Some(line) = lines.next() {
        if line.starts_with(&header) {
            found = true;
            // Skip the heading itself; optional date on same line is omitted.
            continue;
        }
        if found {
            if line.starts_with("## [") {
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
    }

    if !found {
        bail!("CHANGELOG.md missing '{header}' section");
    }
    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        bail!("CHANGELOG section for {version} is empty");
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_middle_section() {
        let md = "\
# Changelog\n\
\n\
## [Unreleased]\n\
\n\
## [0.5.0] - 2026-08-28\n\
\n\
### Breaking\n\
- no baked URLs\n\
\n\
## [0.4.4] - 2026-08-28\n\
\n\
### Added\n\
- older\n\
";
        let body = extract_section(md, "0.5.0").unwrap();
        assert!(body.contains("### Breaking"));
        assert!(body.contains("no baked URLs"));
        assert!(!body.contains("older"));
        assert!(!body.contains("Unreleased"));
    }
}
