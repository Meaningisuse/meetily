use serde_json::Value;

fn inline(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(inline).collect(),
        Value::Object(block) => block
            .get("text")
            .or_else(|| block.get("content"))
            .map(inline)
            .unwrap_or_default(),
        _ => String::new(),
    }
}
fn blocks(value: &Value) -> String {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|block| {
                    let text = block.get("content").map(inline).unwrap_or_default();
                    let kind = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("paragraph");
                    let line = match kind {
                        "heading" => format!(
                            "{} {}",
                            "#".repeat(
                                block
                                    .pointer("/props/level")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(2)
                                    .clamp(1, 6) as usize
                            ),
                            text
                        ),
                        "bulletListItem" => format!("- {text}"),
                        "numberedListItem" => format!("1. {text}"),
                        "checkListItem" => format!(
                            "- [{}] {text}",
                            if block
                                .pointer("/props/checked")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                            {
                                "x"
                            } else {
                                " "
                            }
                        ),
                        "paragraph" => text,
                        "codeBlock" => format!("```\n{text}\n```"),
                        _ if !text.is_empty() => text,
                        _ => format!("[Unsupported block: {kind}]"),
                    };
                    let children = block.get("children").map(blocks).unwrap_or_default();
                    if children.is_empty() {
                        line
                    } else {
                        format!("{line}\n\n{children}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}
pub fn summary_markdown(raw: &str) -> Option<String> {
    let value: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return (!raw.trim().is_empty()).then(|| raw.to_owned()),
    };
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    if value.is_array() {
        return Some(blocks(&value));
    }
    if let Some(text) = value.get("markdown").and_then(Value::as_str) {
        return Some(text.to_owned());
    }
    if let Some(value) = value.get("summary_json") {
        return Some(blocks(value));
    }
    let object = value.as_object()?;
    let mut keys: Vec<&str> = value
        .get("_section_order")
        .and_then(Value::as_array)
        .map(|v| v.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for key in object.keys() {
        if !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let sections = keys
        .into_iter()
        .filter_map(|key| {
            let section = object.get(key)?;
            let title = section.get("title")?.as_str()?;
            let lines = section
                .get("blocks")?
                .as_array()?
                .iter()
                .filter_map(|b| b.get("content").and_then(Value::as_str))
                .map(|text| format!("- {text}"))
                .collect::<Vec<_>>()
                .join("\n");
            Some(format!("## {title}\n\n{lines}"))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (!sections.is_empty()).then_some(sections)
}
/// Legacy seconds are approximate; never invent timing for missing/invalid values.
pub fn seconds_to_ms(seconds: Option<f64>) -> Option<i64> {
    seconds
        .filter(|s| s.is_finite() && *s >= 0.0 && *s < i64::MAX as f64 / 1000.0)
        .map(|s| (s * 1000.0).round() as i64)
}
