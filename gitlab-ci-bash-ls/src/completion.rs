//! Script-key scaffolds with explicit YAML indentation, independent of the
//! editor's choice of Enter or Tab to confirm a completion.

use serde_json::{Value, json};
use yaml_rust2::{Yaml, YamlLoader};

use crate::{YAML_ITEM_MARKER, extract::RESERVED_KEYS};

pub fn script_items(text: &str, position: &Value, yaml: &Value, snippets: bool) -> Vec<Value> {
    if yaml.get("completion") == Some(&json!(false)) {
        return Vec::new();
    }
    let Some(context) = Context::new(text, position, yaml) else {
        return Vec::new();
    };
    ["script", "before_script", "after_script"]
        .into_iter()
        .filter(|key| key.starts_with(&context.prefix))
        .filter(|key| context.job != "default" || *key != "script")
        .filter(|key| context.properties[*key].is_badvalue())
        .map(|key| {
            let indent = " ".repeat(context.indent);
            let body_indent = " ".repeat(context.indent * 2);
            let cursor = if snippets { "$0" } else { "" };
            json!({
                "label": key,
                "kind": 10,
                "detail": format!("Script block in {}", context.job),
                "filterText": key,
                "insertTextFormat": if snippets { 2 } else { 1 },
                "insertTextMode": 1,
                "textEdit": {
                    "range": {
                        "start": { "line": context.line, "character": 0 },
                        "end": { "line": context.line, "character": context.end },
                    },
                    "newText": format!("{indent}{key}:{}{body_indent}- {cursor}", context.newline),
                },
                "data": { YAML_ITEM_MARKER: true },
            })
        })
        .collect()
}

struct Context {
    line: usize,
    end: usize,
    prefix: String,
    job: String,
    properties: Yaml,
    indent: usize,
    newline: &'static str,
}

impl Context {
    fn new(text: &str, position: &Value, yaml: &Value) -> Option<Self> {
        let line = position["line"].as_u64()? as usize;
        let character = position["character"].as_u64()? as usize;
        let lines: Vec<_> = text.split('\n').collect();
        let current = lines.get(line)?.trim_end_matches('\r');
        let existing_indent = current.len() - current.trim_start_matches(' ').len();
        let word = current.trim_matches(' ');
        // Never replace values, comments, quoted strings, or shell commands.
        if !word.bytes().all(|c| c.is_ascii_alphabetic() || c == b'_')
            || !current.is_ascii()
            || character < existing_indent
            || character > current.len()
        {
            return None;
        }
        let prefix = current[existing_indent..character].to_owned();
        let (header_line, header) =
            lines[..line].iter().enumerate().rev().find(|(_, value)| {
                !value.is_empty() && !value.starts_with([' ', '\t', '\r', '#'])
            })?;
        let header_docs = YamlLoader::load_from_str(header).ok()?;
        let header_map = header_docs.first()?.as_hash()?;
        if header_map.len() != 1 {
            return None;
        }
        let (job, value) = header_map.iter().next()?;
        let job = job.as_str()?;
        if !value.is_null() || RESERVED_KEYS.contains(&job) {
            return None;
        }

        // Parsing the prefix also guards against a job-looking line inside a
        // multiline YAML string. Incomplete earlier YAML is left to the backend.
        let before = lines[..line].join("\n");
        let docs = YamlLoader::load_from_str(&before).ok()?;
        let properties = &docs.last()?[job];
        if !properties.is_hash() && !properties.is_null() {
            return None;
        }
        let job_end = lines[line + 1..]
            .iter()
            .position(|value| !value.is_empty() && !value.starts_with([' ', '\t', '\r', '#']))
            .map_or(lines.len(), |offset| line + 1 + offset);
        let mut job_lines = lines[header_line..job_end].to_vec();
        job_lines[line - header_line] = "";
        // Check the whole current job, including properties below the cursor.
        // An unrelated unfinished later job must not hide duplicate keys.
        let job_docs = YamlLoader::load_from_str(&job_lines.join("\n")).ok()?;
        let complete_properties = &job_docs.first()?[job];
        if !complete_properties.is_hash() && !complete_properties.is_null() {
            return None;
        }
        let infer_indent = |lines: &[&str]| {
            lines
                .iter()
                .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
                .map(|line| line.len() - line.trim_start_matches(' ').len())
                .filter(|&indent| indent > 0)
                .min()
        };
        // Existing job structure wins over settings: a configured four-space
        // unit must not turn a nested variable into a supposed job property.
        let indent = infer_indent(&job_lines)
            .or_else(|| {
                yaml["indentation"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.bytes().all(|c| c == b' '))
                    .map(str::len)
            })
            .or_else(|| infer_indent(&lines))
            .unwrap_or(2);
        if existing_indent == 0 {
            // Reparent only directly below an empty job header, never after a
            // completed job, comment, or blank separator (intentional globals).
            if header_line + 1 != line || !properties.is_null() {
                return None;
            }
        } else if existing_indent != indent {
            return None;
        }

        Some(Context {
            line,
            end: current.len(),
            prefix,
            job: job.to_owned(),
            properties: complete_properties.clone(),
            indent,
            newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
        })
    }
}

pub fn merge(mut result: Value, extra: Vec<Value>) -> Value {
    if extra.is_empty() {
        return result;
    }
    if result.is_array() {
        result = json!({ "items": result, "isIncomplete": true });
    } else if result.is_null() {
        result = json!({ "items": [], "isIncomplete": true });
    }
    if let Some(items) = result.get_mut("items").and_then(Value::as_array_mut) {
        items.retain(|item| !extra.iter().any(|extra| extra["label"] == item["label"]));
        items.extend(extra);
        // The context and edit range must be recomputed as the user keeps typing.
        result["isIncomplete"] = json!(true);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(text: &str) -> Vec<Value> {
        let line = text.split('\n').count() - 1;
        let character = text.split('\n').next_back().unwrap().len();
        script_items(
            text,
            &json!({ "line": line, "character": character }),
            &json!({}),
            true,
        )
    }

    #[test]
    fn completes_root_prefix_under_empty_job() {
        for key in ["script", "before_script", "after_script"] {
            let text = format!("test:\n{key}");
            let found = items(&text);
            assert_eq!(found.len(), 1);
            assert_eq!(
                found[0]["textEdit"]["newText"],
                format!("  {key}:\n    - $0")
            );
            assert_eq!(
                found[0]["textEdit"]["range"]["start"],
                json!({ "line": 1, "character": 0 })
            );
            assert_eq!(found[0]["textEdit"]["range"]["end"]["character"], key.len());
        }
    }

    #[test]
    fn respects_existing_and_configured_indentation() {
        assert_eq!(
            items("test:\n  aft")[0]["textEdit"]["newText"],
            "  after_script:\n    - $0"
        );
        assert_eq!(
            items("test:\n    stage: build\n    aft")[0]["textEdit"]["newText"],
            "    after_script:\n        - $0"
        );
        let found = script_items(
            "test:\naft",
            &json!({"line": 1, "character": 3}),
            &json!({"indentation": "    "}),
            false,
        );
        assert_eq!(
            found[0]["textEdit"]["newText"],
            "    after_script:\n        - "
        );
        assert_eq!(found[0]["insertTextFormat"], 1);
    }

    #[test]
    fn leaves_globals_metadata_values_and_nested_keys_alone() {
        for text in [
            "aft",
            "test:\n\naft",
            "test:\n# global\naft",
            "test:\n  script: echo hi\naft",
            "variables:\naft",
            "stages:\nscr",
            "workflow:\n  aft",
            "test:\n  variables:\n    aft",
            "test:\n  script: echo aft",
            "test:\n  # aft",
            "test:\n  after_script:",
            "test:\n  script: |\n    aft",
            "test:\n\taft",
            "job: {}\naft",
            "value: \"hello\nfake_job:\naft",
        ] {
            assert!(items(text).is_empty(), "unexpected scaffold for {text:?}");
        }
        assert!(
            script_items(
                "test:\naft",
                &json!({"line": 1, "character": 3}),
                &json!({"completion": false}),
                true
            )
            .is_empty()
        );
    }

    #[test]
    fn handles_defaults_quotes_comments_crlf_and_duplicates() {
        let keys: Vec<_> = items("default:\n")
            .into_iter()
            .map(|item| item["label"].clone())
            .collect();
        assert_eq!(keys, ["before_script", "after_script"]);
        assert_eq!(
            items("\"täst😀\": # job\naft")[0]["textEdit"]["newText"],
            "  after_script:\n    - $0"
        );
        let found = script_items(
            "test:\r\naft\r\n",
            &json!({"line": 1, "character": 3}),
            &json!({}),
            true,
        );
        assert_eq!(
            found[0]["textEdit"]["newText"],
            "  after_script:\r\n    - $0"
        );
        assert!(items("test:\n  after_script: echo hi\n  aft").is_empty());
        assert!(
            script_items(
                "test:\n  aft\n  after_script: echo hi\n",
                &json!({"line": 1, "character": 5}),
                &json!({}),
                true
            )
            .is_empty()
        );
    }

    #[test]
    fn settings_do_not_override_existing_job_structure() {
        let text = "test:\n  variables:\n    OTHER: value\n    aft";
        assert!(
            script_items(
                text,
                &json!({"line": 3, "character": 7}),
                &json!({"indentation": "    "}),
                true
            )
            .is_empty()
        );
        let text = "test:\n    stage: build\n  aft";
        assert!(
            script_items(
                text,
                &json!({"line": 2, "character": 5}),
                &json!({"indentation": "  "}),
                true
            )
            .is_empty()
        );
        let text = "test:\n    stage: build\n    aft";
        let found = script_items(
            text,
            &json!({"line": 2, "character": 7}),
            &json!({"indentation": "  "}),
            true,
        );
        assert_eq!(
            found[0]["textEdit"]["newText"],
            "    after_script:\n        - $0"
        );
    }

    #[test]
    fn checks_duplicate_keys_even_with_an_unfinished_later_job() {
        let text = "test:\n  aft\n  after_script:\n    - echo cleanup\nother: [\n";
        assert!(
            script_items(text, &json!({"line": 1, "character": 5}), &json!({}), true).is_empty()
        );
        let text = "test:\n  aft\n  script: echo hi\nother: [\n";
        assert_eq!(
            script_items(text, &json!({"line": 1, "character": 5}), &json!({}), true).len(),
            1
        );
    }

    #[test]
    fn merges_without_duplicate_labels_or_losing_backend_metadata() {
        let extra = items("test:\naft");
        for result in [
            json!(null),
            json!([]),
            json!({"items": [{"label": "after_script"}, {"label": "stage"}], "itemDefaults": {"insertTextFormat": 2}}),
        ] {
            let merged = merge(result.clone(), extra.clone());
            let items = merged["items"].as_array().unwrap();
            assert_eq!(
                items
                    .iter()
                    .filter(|item| item["label"] == "after_script")
                    .count(),
                1
            );
            assert_eq!(merged["isIncomplete"], true);
            if result.is_object() {
                assert_eq!(merged["itemDefaults"], result["itemDefaults"]);
                assert!(items.iter().any(|item| item["label"] == "stage"));
            }
        }
    }
}
