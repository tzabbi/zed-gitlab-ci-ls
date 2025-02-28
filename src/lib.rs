use zed_extension_api as zed;

struct GitLabCiLsExtension;

impl zed::Extension for GitLabCiLsExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _config: zed::LanguageServerConfig,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let path = worktree
            .which("gitlab-ci-ls")
            .ok_or_else(|| "The LSP for gitlab-ci 'gitlab-ci-ls' is not installed".to_string())?;

        Ok(zed::Command {
            command: path,
            args: vec!["serve".to_string()],
            env: Default::default(),
        })
    }
}

zed::register_extension!(GitLabCiLsExtension);
