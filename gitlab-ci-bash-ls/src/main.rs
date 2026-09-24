//! Language server proxy for GitLab CI files, in the spirit of `helm-ls`.
//!
//! - bash-language-server: every script (a job's `before_script` + `script`,
//!   its `after_script`, ...) is opened as a virtual `.sh` document. Positions
//!   of diagnostics, hovers and completions are translated to the YAML file.
//! - yaml-language-server: the YAML file itself is forwarded unchanged, with
//!   GitLab's CI JSON schema, for key completion, hover and validation.

mod completion;
mod extract;
mod rpc;

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use extract::{Script, Source};

const NAME: &str = "gitlab-ci-bash-ls";
const GITLAB_CI_SCHEMA: &str = "https://gitlab.com/gitlab-org/gitlab/-/raw/master/app/assets/javascripts/editor/schema/ci.json";
/// Marks completion items that came from yaml-language-server, which does not resolve items.
const YAML_ITEM_MARKER: &str = "gitlabCiYaml";

const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backend {
    Bash,
    Yaml,
}

impl Backend {
    const ALL: [Backend; 2] = [Backend::Bash, Backend::Yaml];

    fn index(self) -> usize {
        self as usize
    }

    fn program(self) -> &'static str {
        match self {
            Backend::Bash => "bash-language-server",
            Backend::Yaml => "yaml-language-server",
        }
    }

    /// Set by the Zed extension to the resolved binary.
    fn path_env(self) -> &'static str {
        match self {
            Backend::Bash => "BASH_LANGUAGE_SERVER_PATH",
            Backend::Yaml => "YAML_LANGUAGE_SERVER_PATH",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Backend::Bash => &["start"],
            Backend::Yaml => &["--stdio"],
        }
    }

    fn purpose(self) -> &'static str {
        match self {
            Backend::Bash => "script linting, hover and completion",
            Backend::Yaml => "GitLab CI key completion and schema validation",
        }
    }
}

enum Incoming {
    Client(Value),
    ClientClosed,
    Server(Backend, Value),
    ServerClosed(Backend),
}

/// Where a positional request was sent.
enum Target {
    Script(String),
    Yaml,
}

/// Requests sent to a backend that await a response.
enum Pending {
    Initialize,
    Shutdown,
    Hover(Value, Target),
    Completion(Value, Target, Vec<Value>),
    Resolve(Value),
}

#[derive(Default)]
struct Server {
    stdin: Option<ChildStdin>,
    child: Option<Child>,
    /// Whether the server finished initializing.
    ready: bool,
}

struct VirtualDocument {
    uri: String,
    version: i64,
    script: Script,
    /// Last diagnostics from bash-language-server, in script coordinates.
    diagnostics: Vec<Value>,
}

struct Document {
    text: String,
    source: Source,
    scripts: Vec<VirtualDocument>,
    yaml_diagnostics: Vec<Value>,
}

/// Settings from `lsp.gitlab-ci-bash-ls.settings` in Zed.
struct Settings {
    /// Forwarded to bash-language-server as its `bashIde` configuration.
    bash_ide: Value,
    /// Dialect passed to ShellCheck through a `# shellcheck shell=` directive.
    shell: Option<String>,
    /// Forwarded to yaml-language-server as its `yaml` configuration.
    yaml: Value,
}

impl Settings {
    fn from_value(value: Option<&Value>) -> Self {
        let get = |key: &str| value.and_then(|value| value.get(key));

        // Job variables are defined by GitLab, not in the script, so SC2154
        // ("referenced but not assigned") would fire on nearly every variable.
        // Background analysis would scan unrelated workspace scripts.
        let mut bash_ide = json!({
            "backgroundAnalysisMaxFiles": 0,
            "shellcheckArguments": ["--exclude=SC2154"],
        });
        merge(&mut bash_ide, get("bashIde"));

        // Lowest to highest precedence: defaults, the user's settings for Zed's
        // YAML server (passed on by the extension), the GitLab schema, and the
        // user's `yaml` settings for this proxy. The YAML backend only ever sees
        // GitLab CI files, so the schema applies to every document.
        let mut yaml = json!({ "completion": true, "hover": true, "validate": true });
        merge(&mut yaml, get("inheritedYaml"));
        merge(
            &mut yaml,
            Some(&json!({
                "schemaStore": { "enable": false },
                "schemas": { GITLAB_CI_SCHEMA: ["*"] },
            })),
        );
        merge(&mut yaml, get("yaml"));
        let tags = yaml
            .as_object_mut()
            .expect("object")
            .entry("customTags")
            .or_insert_with(|| json!([]));
        if let Some(tags) = tags.as_array_mut()
            && !tags.iter().any(|tag| {
                tag.as_str()
                    .is_some_and(|tag| tag.starts_with("!reference"))
            })
        {
            tags.push(json!("!reference sequence"));
        }

        let shell = get("shell")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|shell| !shell.is_empty())
            .map(str::to_owned);
        Settings {
            bash_ide,
            shell,
            yaml,
        }
    }
}

/// Shallow merge of `source`'s keys into the `target` object.
fn merge(target: &mut Value, source: Option<&Value>) {
    if let (Some(target), Some(source)) =
        (target.as_object_mut(), source.and_then(Value::as_object))
    {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
}

struct Proxy {
    tx: Sender<Incoming>,
    servers: [Server; 2],
    next_id: i64,
    pending: HashMap<i64, (Backend, Pending)>,
    /// Client `initialize` request waiting for this many backends.
    initializing: Option<(Value, usize)>,
    /// Client `shutdown` request waiting for this many backends.
    shutting_down: Option<(Value, usize)>,
    shutdown_requested: bool,
    documents: HashMap<String, Document>,
    settings: Settings,
    snippets: bool,
}

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("{NAME} {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let (tx, rx) = mpsc::channel();
    let client_tx = tx.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(io::stdin().lock());
        while let Ok(Some(message)) = rpc::read(&mut reader) {
            if client_tx.send(Incoming::Client(message)).is_err() {
                return;
            }
        }
        let _ = client_tx.send(Incoming::ClientClosed);
    });

    let mut proxy = Proxy {
        tx,
        servers: Default::default(),
        next_id: 0,
        pending: HashMap::new(),
        initializing: None,
        shutting_down: None,
        shutdown_requested: false,
        documents: HashMap::new(),
        settings: Settings::from_value(None),
        snippets: false,
    };
    for message in rx {
        match message {
            Incoming::Client(message) => proxy.on_client_message(message),
            Incoming::Server(backend, message) => proxy.on_server_message(backend, message),
            Incoming::ServerClosed(backend) => proxy.on_server_closed(backend),
            Incoming::ClientClosed => proxy.exit(),
        }
    }
}

impl Proxy {
    fn server(&mut self, backend: Backend) -> &mut Server {
        &mut self.servers[backend.index()]
    }

    fn ready(&self, backend: Backend) -> bool {
        self.servers[backend.index()].ready
    }

    // --- Messages from Zed -------------------------------------------------

    fn on_client_message(&mut self, message: Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return; // A response; this proxy sends no requests to the client.
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match message.get("id").cloned() {
            Some(id) => self.on_client_request(method, id, params),
            None => self.on_client_notification(method, params),
        }
    }

    fn on_client_request(&mut self, method: &str, id: Value, params: Value) {
        match method {
            "initialize" => self.initialize(id, &params),
            "shutdown" => {
                self.shutdown_requested = true;
                let running: Vec<_> = Backend::ALL
                    .into_iter()
                    .filter(|backend| self.servers[backend.index()].stdin.is_some())
                    .collect();
                if running.is_empty() {
                    self.respond(id, Value::Null);
                    return;
                }
                self.shutting_down = Some((id, running.len()));
                for backend in running {
                    self.request(backend, "shutdown", Value::Null, Pending::Shutdown);
                }
            }
            "textDocument/hover" => self.forward_positional(method, id, params, Pending::Hover),
            "textDocument/completion" => {
                let extra = params["textDocument"]["uri"]
                    .as_str()
                    .and_then(|uri| self.documents.get(uri))
                    .map(|document| {
                        completion::script_items(
                            &document.text,
                            &params["position"],
                            &self.settings.yaml,
                            self.snippets,
                        )
                    })
                    .unwrap_or_default();
                self.forward_positional(method, id, params, |id, target| {
                    Pending::Completion(id, target, extra)
                })
            }
            "completionItem/resolve" => {
                let from_yaml = params
                    .pointer(&format!("/data/{YAML_ITEM_MARKER}"))
                    .is_some();
                if from_yaml || !self.ready(Backend::Bash) {
                    self.respond(id, params);
                } else {
                    self.request(Backend::Bash, method, params, Pending::Resolve(id));
                }
            }
            _ => self.respond_error(id, METHOD_NOT_FOUND, format!("unsupported method {method}")),
        }
    }

    fn on_client_notification(&mut self, method: &str, params: Value) {
        match method {
            "exit" => self.exit(),
            "textDocument/didOpen" => {
                let document = &params["textDocument"];
                if let (Some(uri), Some(text)) =
                    (document["uri"].as_str(), document["text"].as_str())
                {
                    self.sync(uri.to_owned(), text.to_owned());
                }
                self.notify_server(Backend::Yaml, method, params);
            }
            "textDocument/didChange" => {
                // Full sync: the last change holds the whole document.
                let uri = params["textDocument"]["uri"].as_str();
                let text = params["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.last())
                    .and_then(|change| change["text"].as_str());
                if let (Some(uri), Some(text)) = (uri, text) {
                    self.sync(uri.to_owned(), text.to_owned());
                }
                self.notify_server(Backend::Yaml, method, params);
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.close(uri);
                }
                self.notify_server(Backend::Yaml, method, params);
            }
            "workspace/didChangeConfiguration" => {
                self.settings = Settings::from_value(params.get("settings"));
                let bash = json!({ "settings": { "bashIde": self.settings.bash_ide } });
                self.notify_server(Backend::Bash, "workspace/didChangeConfiguration", bash);
                let yaml = json!({ "settings": { "yaml": self.settings.yaml } });
                self.notify_server(Backend::Yaml, "workspace/didChangeConfiguration", yaml);
                // The shell directive is part of the virtual documents.
                let open: Vec<_> = self
                    .documents
                    .iter()
                    .map(|(uri, document)| (uri.clone(), document.text.clone()))
                    .collect();
                for (uri, text) in open {
                    self.sync(uri, text);
                }
            }
            "$/cancelRequest" => {
                let pending = self.pending.iter().find_map(|(id, (backend, pending))| {
                    let client_id = match pending {
                        Pending::Hover(client_id, _)
                        | Pending::Completion(client_id, _, _)
                        | Pending::Resolve(client_id) => client_id,
                        Pending::Initialize | Pending::Shutdown => return None,
                    };
                    (*client_id == params["id"]).then_some((*backend, *id))
                });
                if let Some((backend, id)) = pending {
                    self.notify_server(backend, "$/cancelRequest", json!({ "id": id }));
                }
            }
            _ => {}
        }
    }

    fn initialize(&mut self, id: Value, params: &Value) {
        self.settings = Settings::from_value(params.get("initializationOptions"));
        self.snippets = params
            .pointer("/capabilities/textDocument/completion/completionItem/snippetSupport")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut started = 0;
        for backend in Backend::ALL {
            if self.start_server(backend, params) {
                started += 1;
            }
        }
        if started == 0 {
            self.respond(id, initialize_result());
        } else {
            self.initializing = Some((id, started));
        }
    }

    fn start_server(&mut self, backend: Backend, params: &Value) -> bool {
        let path = std::env::var(backend.path_env())
            .ok()
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| backend.program().to_owned());
        let spawned = Command::new(&path)
            .args(backend.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                self.show_message(
                    2,
                    format!(
                        "{NAME}: could not start `{path}` ({error}). Install {} for {}.",
                        backend.program(),
                        backend.purpose()
                    ),
                );
                return false;
            }
        };

        let stdout = child.stdout.take().expect("piped stdout");
        let server = self.server(backend);
        server.stdin = child.stdin.take();
        server.child = Some(child);
        let tx = self.tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(message)) = rpc::read(&mut reader) {
                if tx.send(Incoming::Server(backend, message)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Incoming::ServerClosed(backend));
        });

        // Settings are pushed with initializationOptions and didChangeConfiguration
        // only. Advertising `workspace.configuration` makes bash-language-server
        // await a configuration request in its `initialized` handler; documents
        // opened meanwhile would not be linted (only the last one is remembered).
        let mut capabilities = json!({
            "workspace": { "didChangeConfiguration": { "dynamicRegistration": false } },
            "textDocument": {
                "synchronization": { "dynamicRegistration": false },
                "publishDiagnostics": { "versionSupport": true },
                "hover": { "contentFormat": ["markdown", "plaintext"] },
            },
        });
        if let Some(completion) = params.pointer("/capabilities/textDocument/completion") {
            capabilities["textDocument"]["completion"] = completion.clone();
        }
        let initialization_options = match backend {
            Backend::Bash => self.settings.bash_ide.clone(),
            Backend::Yaml => Value::Null,
        };
        let server_params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": NAME, "version": env!("CARGO_PKG_VERSION") },
            "rootUri": params.get("rootUri").cloned().unwrap_or(Value::Null),
            "rootPath": params.get("rootPath").cloned().unwrap_or(Value::Null),
            "workspaceFolders": params.get("workspaceFolders").cloned().unwrap_or(Value::Null),
            "capabilities": capabilities,
            "initializationOptions": initialization_options,
        });
        self.request(backend, "initialize", server_params, Pending::Initialize);
        true
    }

    /// Sends requests inside a script to bash-language-server and all others
    /// to yaml-language-server.
    fn forward_positional(
        &mut self,
        method: &str,
        id: Value,
        mut params: Value,
        pending: impl FnOnce(Value, Target) -> Pending,
    ) {
        let Some(uri) = params["textDocument"]["uri"].as_str().map(str::to_owned) else {
            self.respond(id, Value::Null);
            return;
        };
        let script = (|| {
            let line = params["position"]["line"].as_u64()? as usize;
            let character = params["position"]["character"].as_u64()? as usize;
            self.locate(&uri, line, character)
        })();
        match script {
            Some((virtual_uri, position)) => {
                if !self.ready(Backend::Bash) {
                    self.respond(id, Value::Null);
                    return;
                }
                params["textDocument"] = json!({ "uri": virtual_uri });
                params["position"] = position;
                self.request(
                    Backend::Bash,
                    method,
                    params,
                    pending(id, Target::Script(virtual_uri)),
                );
            }
            None if method == "textDocument/completion"
                && self.settings.yaml.get("completion") == Some(&json!(false)) =>
            {
                self.respond(id, Value::Null);
            }
            None if self.ready(Backend::Yaml) && self.documents.contains_key(&uri) => {
                self.request(Backend::Yaml, method, params, pending(id, Target::Yaml));
            }
            None => self.respond(id, Value::Null),
        }
    }

    fn locate(&self, uri: &str, line: usize, character: usize) -> Option<(String, Value)> {
        let document = self.documents.get(uri)?;
        let pos = document.source.pos(line, character);
        document.scripts.iter().find_map(|virtual_document| {
            let offset = virtual_document.script.offset_of_source(pos)?;
            let (line, character) = virtual_document.script.position(offset);
            Some((
                virtual_document.uri.clone(),
                json!({ "line": line, "character": character }),
            ))
        })
    }

    /// Re-extracts the scripts of a YAML document and brings the virtual
    /// documents in bash-language-server up to date.
    fn sync(&mut self, uri: String, text: String) {
        let (source, scripts) = extract::parse(&text);
        let (mut previous, yaml_diagnostics) = match self.documents.remove(&uri) {
            Some(document) => (
                document
                    .scripts
                    .into_iter()
                    .map(|virtual_document| (virtual_document.uri.clone(), virtual_document))
                    .collect(),
                document.yaml_diagnostics,
            ),
            None => (HashMap::new(), Vec::new()),
        };

        let mut current = Vec::with_capacity(scripts.len());
        for (index, script) in scripts.into_iter().enumerate() {
            let script = match &self.settings.shell {
                Some(shell) => script.prepend_line(&format!("# shellcheck shell={shell}")),
                None => script,
            };
            let virtual_uri = virtual_uri(&uri, index, &script.name);
            match previous.remove(&virtual_uri) {
                Some(mut virtual_document) => {
                    if virtual_document.script.text != script.text {
                        virtual_document.version += 1;
                        // They refer to the old text; mapping them with the new
                        // script would misplace them until fresh ones arrive.
                        virtual_document.diagnostics.clear();
                        self.notify_server(
                            Backend::Bash,
                            "textDocument/didChange",
                            json!({
                                "textDocument": { "uri": virtual_uri, "version": virtual_document.version },
                                "contentChanges": [{ "text": script.text }],
                            }),
                        );
                    }
                    virtual_document.script = script;
                    current.push(virtual_document);
                }
                None => {
                    self.notify_server(
                        Backend::Bash,
                        "textDocument/didOpen",
                        json!({
                            "textDocument": {
                                "uri": virtual_uri,
                                "languageId": "shellscript",
                                "version": 1,
                                "text": script.text,
                            },
                        }),
                    );
                    current.push(VirtualDocument {
                        uri: virtual_uri,
                        version: 1,
                        script,
                        diagnostics: Vec::new(),
                    });
                }
            }
        }
        for virtual_uri in previous.into_keys() {
            self.notify_server(
                Backend::Bash,
                "textDocument/didClose",
                json!({ "textDocument": { "uri": virtual_uri } }),
            );
        }

        self.documents.insert(
            uri.clone(),
            Document {
                text,
                source,
                scripts: current,
                yaml_diagnostics,
            },
        );
        // Positions of unchanged scripts may have moved.
        self.publish_diagnostics(&uri);
    }

    fn close(&mut self, uri: &str) {
        let Some(document) = self.documents.remove(uri) else {
            return;
        };
        for virtual_document in document.scripts {
            self.notify_server(
                Backend::Bash,
                "textDocument/didClose",
                json!({ "textDocument": { "uri": virtual_document.uri } }),
            );
        }
        self.notify_client(
            "textDocument/publishDiagnostics",
            json!({ "uri": uri, "diagnostics": [] }),
        );
    }

    /// Publishes the YAML diagnostics plus the mapped script diagnostics.
    fn publish_diagnostics(&mut self, uri: &str) {
        let Some(document) = self.documents.get(uri) else {
            return;
        };
        let scripts = document.scripts.iter().flat_map(|virtual_document| {
            virtual_document
                .diagnostics
                .iter()
                .filter_map(|diagnostic| {
                    let range = map_range(
                        &diagnostic["range"],
                        &virtual_document.script,
                        &document.source,
                    )?;
                    let mut diagnostic = diagnostic.clone();
                    diagnostic["range"] = range;
                    if let Some(diagnostic) = diagnostic.as_object_mut() {
                        diagnostic.remove("relatedInformation");
                    }
                    Some(diagnostic)
                })
        });
        let diagnostics: Vec<Value> = document
            .yaml_diagnostics
            .iter()
            .cloned()
            .chain(scripts)
            .collect();
        self.notify_client(
            "textDocument/publishDiagnostics",
            json!({ "uri": uri, "diagnostics": diagnostics }),
        );
    }

    // --- Messages from the backends ----------------------------------------

    fn on_server_message(&mut self, backend: Backend, message: Value) {
        let method = message.get("method").and_then(Value::as_str);
        match (method, message.get("id").cloned()) {
            (Some(method), Some(id)) => {
                let result = match method {
                    "workspace/configuration" => {
                        let items = message["params"]["items"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        Value::Array(
                            items
                                .iter()
                                .map(|item| match (backend, item["section"].as_str()) {
                                    (Backend::Bash, Some("bashIde")) => {
                                        self.settings.bash_ide.clone()
                                    }
                                    (Backend::Yaml, Some("yaml")) => self.settings.yaml.clone(),
                                    _ => Value::Null,
                                })
                                .collect(),
                        )
                    }
                    _ => Value::Null,
                };
                self.send_server(
                    backend,
                    json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                );
            }
            (Some("textDocument/publishDiagnostics"), None) => match backend {
                Backend::Bash => self.on_script_diagnostics(&message["params"]),
                Backend::Yaml => self.on_yaml_diagnostics(&message["params"]),
            },
            (Some("window/logMessage" | "window/showMessage"), None) => {
                self.send_client(message);
            }
            (Some(_), None) => {}
            (None, Some(id)) => {
                let pending = id.as_i64().and_then(|id| self.pending.remove(&id));
                if let Some((backend, pending)) = pending {
                    self.complete(backend, pending, Some(&message));
                }
            }
            (None, None) => {}
        }
    }

    fn on_script_diagnostics(&mut self, params: &Value) {
        let Some(virtual_uri) = params["uri"].as_str() else {
            return;
        };
        let diagnostics = params["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut updated = None;
        for (uri, document) in &mut self.documents {
            let Some(virtual_document) = document
                .scripts
                .iter_mut()
                .find(|virtual_document| virtual_document.uri == virtual_uri)
            else {
                continue;
            };
            // Diagnostics for an older script text would map to wrong positions.
            if params["version"]
                .as_i64()
                .is_none_or(|version| version == virtual_document.version)
            {
                virtual_document.diagnostics = diagnostics;
                updated = Some(uri.clone());
            }
            break;
        }
        if let Some(uri) = updated {
            self.publish_diagnostics(&uri);
        }
    }

    fn on_yaml_diagnostics(&mut self, params: &Value) {
        let Some(uri) = params["uri"].as_str() else {
            return;
        };
        let Some(document) = self.documents.get_mut(uri) else {
            return;
        };
        document.yaml_diagnostics = params["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let uri = uri.to_owned();
        self.publish_diagnostics(&uri);
    }

    /// Finishes a request to a backend. `response` is `None` if the backend
    /// is unavailable or exited before answering.
    fn complete(&mut self, backend: Backend, pending: Pending, response: Option<&Value>) {
        let result = response
            .and_then(|response| response.get("result"))
            .cloned()
            .unwrap_or(Value::Null);
        let error = response.and_then(|response| response.get("error")).cloned();
        match pending {
            Pending::Initialize => {
                match (response, error) {
                    (Some(_), None) => {
                        self.server(backend).ready = true;
                        self.notify_server(backend, "initialized", json!({}));
                        if backend == Backend::Yaml {
                            let settings = json!({ "settings": { "yaml": self.settings.yaml } });
                            self.notify_server(
                                backend,
                                "workspace/didChangeConfiguration",
                                settings,
                            );
                        }
                    }
                    (_, error) => {
                        let reason =
                            error.map_or_else(|| "it exited".to_owned(), |error| error.to_string());
                        self.show_message(
                            2,
                            format!(
                                "{NAME}: {} failed to initialize: {reason}",
                                backend.program()
                            ),
                        );
                        self.stop_server(backend);
                    }
                }
                if let Some((id, remaining)) = self.initializing.take() {
                    if remaining > 1 {
                        self.initializing = Some((id, remaining - 1));
                    } else {
                        self.respond(id, initialize_result());
                    }
                }
            }
            Pending::Shutdown => {
                if let Some((id, remaining)) = self.shutting_down.take() {
                    if remaining > 1 {
                        self.shutting_down = Some((id, remaining - 1));
                    } else {
                        self.respond(id, Value::Null);
                    }
                }
            }
            Pending::Resolve(id) => self.respond_with(id, result, error),
            Pending::Hover(id, Target::Yaml) => self.respond_with(id, result, error),
            Pending::Hover(id, Target::Script(virtual_uri)) => {
                let result = self.map_hover(result, &virtual_uri);
                self.respond_with(id, result, error);
            }
            Pending::Completion(id, target, extra) => {
                let mut result = self.map_completion(result, &target);
                if matches!(target, Target::Yaml) && error.is_none() {
                    result = completion::merge(result, extra);
                }
                self.respond_with(id, result, error);
            }
        }
    }

    fn on_server_closed(&mut self, backend: Backend) {
        let was_running = self.servers[backend.index()].stdin.is_some();
        self.stop_server(backend);
        let ids: Vec<i64> = self
            .pending
            .iter()
            .filter(|(_, (pending_backend, _))| *pending_backend == backend)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some((backend, pending)) = self.pending.remove(&id) {
                self.complete(backend, pending, None);
            }
        }
        if was_running && !self.shutdown_requested {
            self.show_message(
                1,
                format!("{NAME}: {} exited unexpectedly", backend.program()),
            );
        }
    }

    fn find_script(&self, virtual_uri: &str) -> Option<(&Source, &Script)> {
        self.documents.values().find_map(|document| {
            let virtual_document = document
                .scripts
                .iter()
                .find(|virtual_document| virtual_document.uri == virtual_uri)?;
            Some((&document.source, &virtual_document.script))
        })
    }

    fn map_hover(&self, mut hover: Value, virtual_uri: &str) -> Value {
        if let Some(hover) = hover.as_object_mut() {
            let range = hover
                .remove("range")
                .zip(self.find_script(virtual_uri))
                .and_then(|(range, (source, script))| map_range(&range, script, source));
            if let Some(range) = range {
                hover.insert("range".to_owned(), range);
            }
        }
        hover
    }

    fn map_completion(&self, mut completion: Value, target: &Target) -> Value {
        let items = match completion {
            Value::Array(ref mut items) => Some(items),
            Value::Object(ref mut list) => list.get_mut("items").and_then(Value::as_array_mut),
            _ => None,
        };
        let script = match target {
            Target::Script(virtual_uri) => Some(self.find_script(virtual_uri)),
            Target::Yaml => None,
        };
        for item in items.into_iter().flatten() {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            match script {
                Some(script) => map_completion_item(item, script),
                None => {
                    let data = item.entry("data").or_insert_with(|| json!({}));
                    if let Some(data) = data.as_object_mut() {
                        data.insert(YAML_ITEM_MARKER.to_owned(), json!(true));
                    }
                }
            }
        }
        completion
    }

    // --- Plumbing ----------------------------------------------------------

    fn request(&mut self, backend: Backend, method: &str, params: Value, pending: Pending) {
        if self.servers[backend.index()].stdin.is_none() {
            self.complete(backend, pending, None);
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.pending.insert(id, (backend, pending));
        self.send_server(
            backend,
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
    }

    fn notify_server(&mut self, backend: Backend, method: &str, params: Value) {
        if self.ready(backend) {
            self.send_server(
                backend,
                json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            );
        }
    }

    fn send_server(&mut self, backend: Backend, message: Value) {
        let server = self.server(backend);
        if let Some(stdin) = &mut server.stdin
            && rpc::write(stdin, &message).is_err()
        {
            server.stdin = None;
            server.ready = false;
        }
    }

    fn stop_server(&mut self, backend: Backend) {
        let server = self.server(backend);
        server.stdin = None;
        server.ready = false;
        if let Some(mut child) = server.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn send_client(&mut self, message: Value) {
        // If Zed has gone away, the stdin reader will end the process.
        let _ = rpc::write(&mut io::stdout().lock(), &message);
    }

    fn notify_client(&mut self, method: &str, params: Value) {
        self.send_client(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn respond(&mut self, id: Value, result: Value) {
        self.send_client(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn respond_with(&mut self, id: Value, result: Value, error: Option<Value>) {
        match error {
            Some(error) => self.send_client(json!({ "jsonrpc": "2.0", "id": id, "error": error })),
            None => self.respond(id, result),
        }
    }

    fn respond_error(&mut self, id: Value, code: i64, message: String) {
        self.send_client(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }));
    }

    /// `kind`: 1 = error, 2 = warning (LSP `MessageType`).
    fn show_message(&mut self, kind: u8, message: String) {
        eprintln!("{message}");
        self.notify_client(
            "window/showMessage",
            json!({ "type": kind, "message": message }),
        );
    }

    fn exit(&mut self) -> ! {
        for backend in Backend::ALL {
            if self.servers[backend.index()].stdin.is_some() {
                self.send_server(backend, json!({ "jsonrpc": "2.0", "method": "exit" }));
                self.server(backend).stdin = None;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        for server in &mut self.servers {
            if let Some(mut child) = server.child.take() {
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        std::process::exit(if self.shutdown_requested { 0 } else { 1 });
    }
}

fn initialize_result() -> Value {
    json!({
        "capabilities": {
            "textDocumentSync": { "openClose": true, "change": 1 },
            "hoverProvider": true,
            // bash-language-server also triggers on `-`, which starts every YAML list item.
            "completionProvider": { "triggerCharacters": ["$", "{"], "resolveProvider": true },
        },
        "serverInfo": { "name": NAME, "version": env!("CARGO_PKG_VERSION") },
    })
}

/// URI of the virtual document for a script, e.g.
/// `file:///repo/.gitlab-ci.yml.3.deploy-script.sh`.
fn virtual_uri(uri: &str, index: usize, name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{uri}.{index}.{slug}.sh")
}

fn map_range(range: &Value, script: &Script, source: &Source) -> Option<Value> {
    let offset = |position: &Value| {
        let line = position["line"].as_u64()? as usize;
        let character = position["character"].as_u64()? as usize;
        Some(script.offset(line, character))
    };
    let (from, to) = script.source_range(offset(&range["start"])?, offset(&range["end"])?)?;
    Some(json!({
        "start": { "line": from.line, "character": source.utf16_col(from) },
        "end": { "line": to.line, "character": source.utf16_col(to) },
    }))
}

/// Moves completion edits into YAML coordinates. Edits that cannot be mapped
/// fall back to plain insert text; extra edits elsewhere in the script are dropped.
fn map_completion_item(item: &mut Map<String, Value>, script: Option<(&Source, &Script)>) {
    item.remove("additionalTextEdits");
    let Some(edit) = item.remove("textEdit") else {
        return;
    };
    let new_text = edit.get("newText").cloned().unwrap_or(Value::Null);
    let map = |key: &str| {
        let (source, script) = script?;
        map_range(edit.get(key)?, script, source)
    };
    let mapped = if edit.get("range").is_some() {
        map("range").map(|range| json!({ "range": range, "newText": new_text }))
    } else {
        map("insert")
            .zip(map("replace"))
            .map(|(insert, replace)| json!({ "insert": insert, "replace": replace, "newText": new_text }))
    };
    match mapped {
        Some(edit) => {
            item.insert("textEdit".to_owned(), edit);
        }
        None => {
            item.entry("insertText").or_insert(new_text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_settings_pin_the_gitlab_schema_over_inherited_settings() {
        let settings = Settings::from_value(Some(&json!({
            "inheritedYaml": {
                "schemaStore": { "enable": true },
                "schemas": { "kubernetes": "*.yaml" },
                "format": { "singleQuote": false },
                "validate": false,
            },
        })));
        assert_eq!(settings.yaml["schemaStore"], json!({ "enable": false }));
        assert_eq!(settings.yaml["schemas"], json!({ GITLAB_CI_SCHEMA: ["*"] }));
        assert_eq!(settings.yaml["format"], json!({ "singleQuote": false }));
        assert_eq!(settings.yaml["validate"], false);
        assert_eq!(settings.yaml["customTags"], json!(["!reference sequence"]));
    }

    #[test]
    fn user_yaml_settings_override_everything() {
        let settings = Settings::from_value(Some(&json!({
            "yaml": { "schemas": { "file:///local.json": "*" }, "customTags": ["!reference sequence", "!vault"] },
        })));
        assert_eq!(
            settings.yaml["schemas"],
            json!({ "file:///local.json": "*" })
        );
        assert_eq!(
            settings.yaml["customTags"],
            json!(["!reference sequence", "!vault"])
        );
        assert_eq!(
            settings.bash_ide["shellcheckArguments"],
            json!(["--exclude=SC2154"])
        );
    }
}
