use super::processor::{clean_llm_markdown_detailed, contains_reasoning_marker};

fn blocknote_has_visible_text(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.iter().any(blocknote_has_visible_text),
        serde_json::Value::Object(object) => {
            object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
                || object
                    .get("content")
                    .is_some_and(blocknote_has_visible_text)
                || object
                    .get("children")
                    .is_some_and(blocknote_has_visible_text)
        }
        _ => false,
    }
}

fn legacy_section_has_visible_text(value: &serde_json::Value) -> bool {
    value
        .get("blocks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|content| !content.trim().is_empty())
            })
        })
}

fn summary_contains_reasoning_marker(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.iter().any(summary_contains_reasoning_marker),
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| match key.as_str() {
            "markdown" | "text" | "content" => {
                value.as_str().is_some_and(contains_reasoning_marker)
                    || value.is_array() && summary_contains_reasoning_marker(value)
            }
            "summary_json" | "children" => summary_contains_reasoning_marker(value),
            _ => false,
        }),
        _ => false,
    }
}

pub(crate) fn summary_is_renderable(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if summary_contains_reasoning_marker(value) {
        return false;
    }
    if let Some(markdown) = object.get("markdown") {
        return markdown
            .as_str()
            .is_some_and(|markdown| !clean_llm_markdown_detailed(markdown).markdown.is_empty());
    }
    if let Some(blocks) = object.get("summary_json") {
        return blocknote_has_visible_text(blocks);
    }
    object
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "MeetingName" | "_section_order"))
        .any(|(_, value)| legacy_section_has_visible_text(value))
}
