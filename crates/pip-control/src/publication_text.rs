//! Human-facing GitHub prose. Structured worker evidence stays in the ledger.

pub(crate) fn prose(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('@', "&#64;")
}

pub(crate) fn bullets<T: AsRef<str>>(items: impl IntoIterator<Item = T>, empty: &str) -> String {
    let lines = items
        .into_iter()
        .map(|item| prose(item.as_ref()))
        .filter(|item| !item.is_empty())
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        empty.into()
    } else {
        lines.join("\n")
    }
}

/// Only explicit human summaries are suitable for publication. Do not flatten
/// arbitrary evidence objects: they contain paths, hashes and internal state.
pub(crate) fn summaries(value: &serde_json::Value) -> Vec<&str> {
    match value {
        serde_json::Value::String(text) => vec![text],
        serde_json::Value::Array(items) => items.iter().flat_map(summaries).collect(),
        serde_json::Value::Object(fields) => ["summary", "description", "title", "assessment"]
            .into_iter()
            .find_map(|key| fields.get(key).and_then(serde_json::Value::as_str))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_text_cannot_inject_markers_links_mentions_or_code_fences() {
        let text = prose("<!-- marker -->\n@someone [click](https://example.invalid) ```json");
        assert!(!text.contains("<!--"));
        assert!(!text.contains('@'));
        assert!(!text.contains("[click]"));
        assert!(!text.contains("```json"));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn structured_entries_publish_only_explicit_summaries() {
        let value = serde_json::json!([
            {"title":"Wait for the dependency fix", "artifact_directory":"/private", "sha256":"opaque"},
            "Confirm behavior with the issue author",
            {"internal_state":"not prose"},
        ]);
        assert_eq!(
            summaries(&value),
            [
                "Wait for the dependency fix",
                "Confirm behavior with the issue author"
            ]
        );
        assert_eq!(
            bullets(Vec::<String>::new(), "None reported."),
            "None reported."
        );
    }
}
