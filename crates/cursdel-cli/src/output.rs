//! Renders a finished operation's [`Summary`] as text or `--json`, and
//! honours `--quiet`/`--log`. See `docs/JSON_OUTPUT.md` for the schema.

use std::path::Path;

use cursdel_core::report::Summary;

pub fn render(summary: &Summary, json: bool, quiet: bool, log_path: Option<&Path>) {
    let rendered = if json {
        serde_json::to_string_pretty(&summary.to_json())
            .unwrap_or_else(|e| format!("{{\"error\":\"failed to encode JSON report: {e}\"}}"))
    } else {
        summary.to_text()
    };

    if json {
        println!("{rendered}");
    } else if !quiet {
        print!("{rendered}");
    } else if !summary.failures.is_empty() {
        for failure in &summary.failures {
            eprintln!(
                "FAILED [{}] {}: {}",
                failure.category,
                failure.path.display(),
                failure.message
            );
        }
    }

    if let Some(path) = log_path {
        if let Err(e) = std::fs::write(path, &rendered) {
            eprintln!(
                "Warning: could not write --log file {}: {e}",
                path.display()
            );
        }
    }
}
