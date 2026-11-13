use zed::serde_json::{Map, Value};
use zed::settings::LspSettings;
use zed_extension_api as zed;

mod shellcheck;

struct GitLabCiLsExtension {
    shellcheck_path: Option<String>,
}

impl zed::Extension for GitLabCiLsExtension {
    fn new() -> Self {
        Self {
            shellcheck_path: None,
        }
    }
    fn language_server_command(
        &mut self,
        _language_server_id: &zed_extension_api::LanguageServerId,
        worktree: &zed_extension_api::Worktree,
    ) -> zed_extension_api::Result<zed_extension_api::Command> {
        let path = worktree
            .which("gitlab-ci-ls")
            .ok_or_else(|| "The LSP for gitlab-ci 'gitlab-ci-ls' is not installed".to_string())?;

        Ok(zed::Command {
            command: path,
            args: vec![],
            env: Default::default(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &zed_extension_api::LanguageServerId,
        worktree: &zed_extension_api::Worktree,
    ) -> zed_extension_api::Result<Option<Value>> {
        let mut options = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.initialization_options)
            .unwrap_or(Value::Null);

        let configured = &options["options"]["shellcheck"];
        let enabled = configured["enabled"].as_bool().unwrap_or(true);

        // gitlab-ci-ls lints scripts with ShellCheck, so make sure it has one.
        // Without it the server still works, just without those diagnostics.
        if enabled
            && configured["path"].is_null()
            && let Ok(path) =
                shellcheck::binary_path(&mut self.shellcheck_path, language_server_id, worktree)
        {
            set_shellcheck_path(&mut options, path);
        }

        Ok((!options.is_null()).then_some(options))
    }
}

fn set_shellcheck_path(options: &mut Value, path: String) {
    let mut value = options;
    for key in ["options", "shellcheck"] {
        if value.is_null() {
            *value = Value::Object(Map::new());
        }
        let Some(object) = value.as_object_mut() else {
            return;
        };
        value = object.entry(key).or_insert(Value::Null);
    }

    if value.is_null() {
        *value = Value::Object(Map::new());
    }
    if let Some(shellcheck) = value.as_object_mut() {
        shellcheck.insert("path".to_string(), Value::String(path));
    }
}

zed::register_extension!(GitLabCiLsExtension);
