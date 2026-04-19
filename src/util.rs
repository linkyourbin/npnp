use serde_json::Value;

pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();

    let trimmed = cleaned.trim_matches(|ch| ch == ' ' || ch == '.').trim();
    if trimmed.is_empty() {
        "component".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn nested_value<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

pub fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

pub fn nested_string(value: &Value, path: &[&str]) -> Option<String> {
    nested_value(value, path).and_then(value_to_string)
}

pub fn split_obj_and_mtl(content: &str) -> (String, String) {
    let lines: Vec<&str> = content.lines().collect();
    let mut mtl_lines = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("newmtl") {
            mtl_lines.push(line.to_string());
            let mut j = i + 1;
            while j < lines.len() {
                let next_line = lines[j];
                let token = next_line.split_whitespace().next().unwrap_or_default();
                if matches!(
                    token,
                    "newmtl" | "v" | "vt" | "vn" | "f" | "o" | "g" | "s" | "usemtl" | "mtllib"
                ) {
                    break;
                }
                mtl_lines.push(next_line.to_string());
                j += 1;
            }
        }
        i += 1;
    }

    let mut obj_text = lines.join("\n");
    if !obj_text.ends_with('\n') {
        obj_text.push('\n');
    }

    let mut mtl_text = mtl_lines.join("\n");
    if !mtl_text.is_empty() && !mtl_text.ends_with('\n') {
        mtl_text.push('\n');
    }

    (obj_text, mtl_text)
}

#[cfg(test)]
mod tests {
    use super::{sanitize_filename, split_obj_and_mtl};

    #[test]
    fn sanitizes_windows_unsafe_characters() {
        assert_eq!(sanitize_filename("A<B>:C*.step"), "A_B__C_.step");
        assert_eq!(sanitize_filename(" .. "), "component");
    }

    #[test]
    fn splits_embedded_mtl_sections() {
        let input = "newmtl body\nKd 0.8 0.8 0.8\nv 0 0 0\nf 1 1 1\n";
        let (obj_text, mtl_text) = split_obj_and_mtl(input);
        assert!(obj_text.contains("v 0 0 0"));
        assert!(mtl_text.contains("newmtl body"));
        assert!(mtl_text.contains("Kd 0.8 0.8 0.8"));
    }
}
