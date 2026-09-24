//! End-to-end test against real bash-language-server, ShellCheck and
//! yaml-language-server. Skipped when one of them is not installed.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const URI: &str = "file:///tmp/gitlab-ci-bash-ls-test/.gitlab-ci.yml";

const YAML: &str = "\
stages: [build, deploy]

build:
  script:
    - ls $BUILD_DIR

deploy:
  script:
    - export GREETING=hi
    - echo \"$GREETING\"
    - |
      echo \"jojo\"
      echo $HI

hallo:
  s
";

/// A tiny stand-in for GitLab's CI schema, so the test runs offline.
const SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "stages": { "type": "array" },
    "before_script": { "type": ["string", "array"] },
    "after_script": { "type": ["string", "array"] },
    "default": {
      "type": ["object", "null", "string"],
      "properties": {
        "before_script": { "type": ["string", "array"] },
        "after_script": { "type": ["string", "array"] }
      },
      "additionalProperties": false
    }
  },
  "additionalProperties": {
    "type": ["object", "null", "string"],
    "properties": {
      "stage": { "type": "string", "description": "Job stage" },
      "script": { "type": ["string", "array"] },
      "before_script": { "type": ["string", "array"] },
      "after_script": { "type": ["string", "array"] }
    }
  }
}"#;

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

fn send(stdin: &mut ChildStdin, message: Value) {
    let body = serde_json::to_vec(&message).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn read_messages(stdout: impl Read + Send + 'static) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                return;
            }
        }
    });
    rx
}

fn wait_for(rx: &Receiver<Value>, mut accept: impl FnMut(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let message = rx
            .recv_timeout(remaining)
            .expect("timed out waiting for message");
        if accept(&message) {
            return message;
        }
    }
}

fn response(rx: &Receiver<Value>, id: i64) -> Value {
    wait_for(rx, |message| {
        message["id"] == id && message.get("method").is_none()
    })
}

fn labels(response: &Value) -> Vec<String> {
    let result = &response["result"];
    let items = result.as_array().or_else(|| result["items"].as_array());
    items
        .into_iter()
        .flatten()
        .filter_map(|item| item["label"].as_str().map(str::to_owned))
        .collect()
}

fn request(
    stdin: &mut ChildStdin,
    rx: &Receiver<Value>,
    next_id: &mut i64,
    method: &str,
    params: Value,
) -> Value {
    let id = *next_id;
    *next_id += 1;
    send(
        stdin,
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
    );
    let result = response(rx, id);
    assert!(result.get("error").is_none(), "{method}: {result:#}");
    result
}

fn change_and_complete(
    stdin: &mut ChildStdin,
    rx: &Receiver<Value>,
    next_id: &mut i64,
    text: &str,
    position: Value,
) -> Value {
    send(
        stdin,
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": URI, "version": *next_id },
                "contentChanges": [{ "text": text }],
            },
        }),
    );
    // Requests follow didChange on the same stream; no diagnostic timer or sleep is needed.
    request(
        stdin,
        rx,
        next_id,
        "textDocument/completion",
        json!({ "textDocument": { "uri": URI }, "position": position }),
    )
}

fn end_position(text: &str) -> Value {
    let line = text.split('\n').count() - 1;
    let character = text.rsplit('\n').next().unwrap().encode_utf16().count();
    json!({ "line": line, "character": character })
}

fn unique_item<'a>(response: &'a Value, label: &str) -> &'a Value {
    let result = &response["result"];
    let items = result.as_array().or_else(|| result["items"].as_array());
    let matching: Vec<_> = items
        .into_iter()
        .flatten()
        .filter(|item| item["label"] == label)
        .collect();
    assert_eq!(matching.len(), 1, "expected one {label}: {response:#}");
    matching[0]
}

fn apply_text_edit(text: &str, item: &Value) -> String {
    let offset = |position: &Value| {
        let line = position["line"].as_u64().unwrap() as usize;
        let character = position["character"].as_u64().unwrap() as usize;
        let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let mut utf16 = 0;
        for (byte, ch) in text[start..].char_indices() {
            if utf16 == character {
                return start + byte;
            }
            assert_ne!(ch, '\n', "position past end of line: {position}");
            utf16 += ch.len_utf16();
        }
        assert_eq!(utf16, character, "invalid UTF-16 position: {position}");
        text.len()
    };
    let edit = &item["textEdit"];
    let mut edited = text.to_owned();
    edited.replace_range(
        offset(&edit["range"]["start"])..offset(&edit["range"]["end"]),
        edit["newText"].as_str().expect("completion textEdit"),
    );
    edited
}

fn assert_scaffold(
    stdin: &mut ChildStdin,
    rx: &Receiver<Value>,
    next_id: &mut i64,
    text: &str,
    label: &str,
    indent: &str,
    snippets: bool,
) {
    let position = end_position(text);
    let result = change_and_complete(stdin, rx, next_id, text, position.clone());
    let item = unique_item(&result, label);
    let new_text = format!(
        "{indent}{label}:\n{indent}{indent}- {}",
        if snippets { "$0" } else { "" }
    );
    assert_eq!(
        item["textEdit"],
        json!({
            "range": { "start": { "line": position["line"], "character": 0 }, "end": position },
            "newText": new_text,
        }),
        "{text:?}: {item:#}"
    );
    if snippets {
        assert_eq!(item["insertTextFormat"], 2, "{item:#}");
    } else {
        assert!(
            item.get("insertTextFormat").is_none() || item["insertTextFormat"] == 1,
            "plain-text client received a snippet: {item:#}"
        );
    }
    assert_eq!(item["data"]["gitlabCiYaml"], true, "{item:#}");
    let resolved = request(stdin, rx, next_id, "completionItem/resolve", item.clone());
    assert_eq!(
        resolved["result"], *item,
        "YAML resolve must preserve the edit"
    );

    let edited = apply_text_edit(text, &resolved["result"]);
    let prefix = &text[..text.rfind('\n').unwrap() + 1];
    assert_eq!(edited, format!("{prefix}{new_text}"));
    // Simulate typing a command at the snippet's final cursor (or the plain-text end).
    let completed = if snippets {
        edited.replace("$0", "echo done")
    } else {
        format!("{edited}echo done")
    };
    let documents = yaml_rust2::YamlLoader::load_from_str(&completed).expect("valid scaffold YAML");
    let owner = prefix
        .lines()
        .find(|line| *line == "test:" || *line == "default:")
        .unwrap();
    assert_eq!(
        documents[0][owner.trim_end_matches(':')][label][0].as_str(),
        Some("echo done"),
        "scaffold must remain a direct child: {completed}"
    );
}

fn configure_yaml(stdin: &mut ChildStdin, schema_uri: &str, mut yaml: Value) {
    yaml["schemas"] = json!({ schema_uri: ["*"] });
    yaml["schemaStore"] = json!({ "enable": false });
    send(
        stdin,
        json!({
            "jsonrpc": "2.0", "method": "workspace/didChangeConfiguration",
            "params": { "settings": { "yaml": yaml } },
        }),
    );
}

fn script_key_regressions(
    stdin: &mut ChildStdin,
    rx: &Receiver<Value>,
    next_id: &mut i64,
    schema_uri: &str,
    snippets: bool,
) {
    for (text, label) in [
        ("test:\naft", "after_script"),
        ("test:\nbef", "before_script"),
        ("test:\nscr", "script"),
        ("test:\n  aft", "after_script"),
        ("test:\n  bef", "before_script"),
        ("test:\n  scr", "script"),
        ("test:\n  stage: test\n  scr", "script"),
        ("default:\naft", "after_script"),
        ("default:\nbef", "before_script"),
        ("default:\n  aft", "after_script"),
        ("default:\n  bef", "before_script"),
    ] {
        assert_scaffold(stdin, rx, next_id, text, label, "  ", snippets);
    }

    // Local script items replace, rather than duplicate, the schema's items; other keys survive.
    let text = "test:\n  s";
    let result = change_and_complete(stdin, rx, next_id, text, end_position(text));
    unique_item(&result, "script");
    unique_item(&result, "stage");

    for text in ["default:\nscr", "default:\n  s"] {
        let result = change_and_complete(stdin, rx, next_id, text, end_position(text));
        assert!(
            !labels(&result).iter().any(|label| label == "script"),
            "{result:#}"
        );
    }

    // Global hooks stay at column zero, even when the preceding job has a script.
    for (text, label) in [
        ("aft", "after_script"),
        ("stages: [test]\nbef", "before_script"),
        ("test:\n\naft", "after_script"),
        ("test:\n# Global hook\nbef", "before_script"),
        ("test:\n  script:\n    - echo done\naft", "after_script"),
        ("test:\n  script:\n    - echo done\nbef", "before_script"),
    ] {
        let result = change_and_complete(stdin, rx, next_id, text, end_position(text));
        let item = unique_item(&result, label);
        let resolved = request(stdin, rx, next_id, "completionItem/resolve", item.clone());
        assert_eq!(resolved["result"], *item);
        let edited = apply_text_edit(text, &resolved["result"]);
        let prefix = &text[..text.rfind('\n').map_or(0, |index| index + 1)];
        assert!(
            edited.starts_with(&format!("{prefix}{label}:")),
            "global hook was reparented: {edited:?} ({item:#})"
        );
    }

    let inferred = "existing:\n    script:\n        - echo done\ntest:\naft";
    assert_scaffold(
        stdin,
        rx,
        next_id,
        inferred,
        "after_script",
        "    ",
        snippets,
    );

    // Explicit indentation overrides the existing job's two-space indentation.
    configure_yaml(stdin, schema_uri, json!({ "indentation": "    " }));
    let explicit = "existing:\n  script:\n    - echo done\ntest:\naft";
    assert_scaffold(
        stdin,
        rx,
        next_id,
        explicit,
        "after_script",
        "    ",
        snippets,
    );
    assert_scaffold(
        stdin,
        rx,
        next_id,
        "test:\n    scr",
        "script",
        "    ",
        snippets,
    );

    configure_yaml(stdin, schema_uri, json!({ "completion": false }));
    for text in ["test:\naft", "test:\n  scr", "default:\n  bef", "aft"] {
        let result = change_and_complete(stdin, rx, next_id, text, end_position(text));
        assert!(
            labels(&result).is_empty(),
            "disabled YAML completion: {result:#}"
        );
    }
    configure_yaml(stdin, schema_uri, json!({}));
    assert_scaffold(
        stdin,
        rx,
        next_id,
        "test:\naft",
        "after_script",
        "  ",
        snippets,
    );
}

#[test]
fn forwards_scripts_to_bash_and_the_file_to_yaml() {
    run_proxy(true);
}

#[test]
fn forwards_scripts_to_bash_and_the_file_to_yaml_without_snippets() {
    run_proxy(false);
}

fn run_proxy(snippets: bool) {
    let backends = ["bash-language-server", "shellcheck", "yaml-language-server"];
    if let Some(missing) = backends.iter().find(|program| !on_path(program)) {
        eprintln!("skipping: {missing} is not installed");
        return;
    }

    let schema = std::env::temp_dir().join(format!(
        "gitlab-ci-bash-ls-schema-{}-{snippets}.json",
        std::process::id()
    ));
    std::fs::write(&schema, SCHEMA).unwrap();
    let schema_uri = format!("file://{}", schema.display());

    let mut child = Command::new(env!("CARGO_BIN_EXE_gitlab-ci-bash-ls"))
        .env("BASH_LANGUAGE_SERVER_PATH", "bash-language-server")
        .env("YAML_LANGUAGE_SERVER_PATH", "yaml-language-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let rx = read_messages(child.stdout.take().unwrap());

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "textDocument": { "completion": { "completionItem": { "snippetSupport": snippets } } },
                },
                "initializationOptions": { "yaml": { "schemas": { schema_uri.as_str(): ["*"] } } },
            },
        }),
    );
    let initialized = response(&rx, 1);
    assert_eq!(initialized["result"]["capabilities"]["hoverProvider"], true);
    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    );
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": URI, "languageId": "yaml", "version": 1, "text": YAML } },
        }),
    );

    // Several scripts are opened right after initialization; all must be linted.
    let shellcheck = |message: &Value| -> Vec<(Value, Value)> {
        message["params"]["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|diagnostic| diagnostic["source"] == "shellcheck")
            .map(|diagnostic| (diagnostic["code"].clone(), diagnostic["range"].clone()))
            .collect()
    };
    let published = wait_for(&rx, |message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == URI
            && shellcheck(message).len() >= 2
    });
    assert_eq!(
        shellcheck(&published),
        [
            (
                json!("SC2086"),
                json!({ "start": { "line": 4, "character": 9 }, "end": { "line": 4, "character": 19 } })
            ),
            (
                json!("SC2086"),
                json!({ "start": { "line": 12, "character": 11 }, "end": { "line": 12, "character": 14 } })
            ),
        ],
        "{published:#}"
    );

    // Hover on `GREETING` in the second list item, defined by the first one.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
            "params": { "textDocument": { "uri": URI }, "position": { "line": 9, "character": 15 } },
        }),
    );
    let hover = response(&rx, 2);
    assert!(
        hover["result"].to_string().contains("GREETING"),
        "unexpected hover: {hover}"
    );

    // Outside of scripts, completion comes from yaml-language-server and the schema.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": URI }, "position": { "line": 15, "character": 3 } },
        }),
    );
    let keys = labels(&response(&rx, 3));
    assert!(
        keys.iter().any(|key| key == "stage"),
        "no `stage` key in {keys:?}"
    );

    // Inside a script, completion comes from bash-language-server.
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": URI }, "position": { "line": 8, "character": 10 } },
        }),
    );
    let commands = labels(&response(&rx, 4));
    assert!(
        commands.iter().any(|command| command == "export"),
        "no `export` in {commands:?}"
    );
    assert!(
        !commands.iter().any(|command| command == "stage"),
        "YAML keys in a script: {commands:?}"
    );

    let mut next_id = 5;
    // The real Bash backend deterministically completes a variable defined in the same script.
    // The cursor is after a YAML escape, before the closing quote, in source UTF-16 units.
    for escape in ["\\x45", "\\u0045", "\\U00000045"] {
        let text =
            format!("test:\n  script:\n    - export GREETING=hi\n    - \"echo $GRE{escape}\"");
        let mut position = end_position(&text);
        position["character"] = json!(position["character"].as_u64().unwrap() - 1);
        let result = change_and_complete(&mut stdin, &rx, &mut next_id, &text, position);
        let item = unique_item(&result, "GREETING");
        assert!(item["data"].get("gitlabCiYaml").is_none(), "{result:#}");
    }

    script_key_regressions(&mut stdin, &rx, &mut next_id, &schema_uri, snippets);
    request(&mut stdin, &rx, &mut next_id, "shutdown", Value::Null);
    send(&mut stdin, json!({ "jsonrpc": "2.0", "method": "exit" }));
    assert!(child.wait().unwrap().success());
    let _ = std::fs::remove_file(schema);
}
