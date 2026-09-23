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
  "properties": { "stages": { "type": "array" } },
  "additionalProperties": {
    "type": ["object", "null", "string"],
    "properties": {
      "stage": { "type": "string", "description": "Job stage" },
      "script": { "type": ["string", "array"] }
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

#[test]
fn forwards_scripts_to_bash_and_the_file_to_yaml() {
    let backends = ["bash-language-server", "shellcheck", "yaml-language-server"];
    if let Some(missing) = backends.iter().find(|program| !on_path(program)) {
        eprintln!("skipping: {missing} is not installed");
        return;
    }

    let schema = std::env::temp_dir().join(format!(
        "gitlab-ci-bash-ls-schema-{}.json",
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
                "capabilities": {},
                "initializationOptions": { "yaml": { "schemas": { schema_uri: ["*"] } } },
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

    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown" }),
    );
    response(&rx, 5);
    send(&mut stdin, json!({ "jsonrpc": "2.0", "method": "exit" }));
    assert!(child.wait().unwrap().success());
    let _ = std::fs::remove_file(schema);
}
