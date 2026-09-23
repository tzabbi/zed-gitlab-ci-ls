use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

const BASH_PROXY: &str = "gitlab-ci-bash-ls";
/// Read by `gitlab-ci-bash-ls` to find the backend language servers.
const BACKENDS: [(&str, &str); 2] = [
    ("bash-language-server", "BASH_LANGUAGE_SERVER_PATH"),
    ("yaml-language-server", "YAML_LANGUAGE_SERVER_PATH"),
];

struct GitLabCiLsExtension;

impl zed::Extension for GitLabCiLsExtension {
    fn new() -> Self {
        Self
    }
    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        if language_server_id.as_ref() == BASH_PROXY {
            return bash_proxy_command(worktree);
        }

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
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(bash_proxy_settings(language_server_id, worktree))
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(bash_proxy_settings(language_server_id, worktree))
    }
}

/// The proxy reads the same `lsp.gitlab-ci-bash-ls.settings` at startup and on changes.
/// The user's settings for Zed's own YAML server are passed along as `inheritedYaml`,
/// so options such as formatting keep applying to GitLab CI files.
fn bash_proxy_settings(
    language_server_id: &LanguageServerId,
    worktree: &zed::Worktree,
) -> Option<zed::serde_json::Value> {
    if language_server_id.as_ref() != BASH_PROXY {
        return None;
    }
    let mut settings = LspSettings::for_worktree(BASH_PROXY, worktree)
        .ok()
        .and_then(|settings| settings.settings)
        .filter(|settings| settings.is_object())
        .unwrap_or_else(|| zed::serde_json::json!({}));
    let inherited_yaml = LspSettings::for_worktree("yaml-language-server", worktree)
        .ok()
        .and_then(|settings| settings.settings)
        .and_then(|mut settings| settings.get_mut("yaml").map(|yaml| yaml.take()));
    if let Some(inherited_yaml) = inherited_yaml {
        settings["inheritedYaml"] = inherited_yaml;
    }
    Some(settings)
}

/// Like the Helm extension does for yaml-language-server, resolve the backend
/// servers here and hand their paths to the proxy.
fn bash_proxy_command(worktree: &zed::Worktree) -> Result<zed::Command> {
    let binary = LspSettings::for_worktree(BASH_PROXY, worktree)
        .ok()
        .and_then(|settings| settings.binary);

    let path = binary
        .as_ref()
        .and_then(|binary| binary.path.clone())
        .or_else(|| worktree.which(BASH_PROXY))
        .ok_or_else(|| {
            format!(
                "'{BASH_PROXY}' is not installed. Build it from this extension's repository with \
                 `cargo install --path {BASH_PROXY}`, or disable it with \
                 \"language_servers\": [\"...\", \"!{BASH_PROXY}\"]."
            )
        })?;

    let mut env = worktree.shell_env();
    for (program, path_env) in BACKENDS {
        if let Some(path) = worktree.which(program) {
            set_env(&mut env, path_env, path);
        }
    }
    let (arguments, user_env) = match binary {
        Some(binary) => (binary.arguments, binary.env),
        None => (None, None),
    };
    for (key, value) in user_env.unwrap_or_default() {
        set_env(&mut env, &key, value);
    }

    Ok(zed::Command {
        command: path,
        args: arguments.unwrap_or_default(),
        env,
    })
}

fn set_env(env: &mut Vec<(String, String)>, key: &str, value: String) {
    env.retain(|(existing, _)| existing != key);
    env.push((key.to_owned(), value));
}

zed::register_extension!(GitLabCiLsExtension);
