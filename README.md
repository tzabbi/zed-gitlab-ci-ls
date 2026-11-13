# zed-gitlab-ci-ls

Zed extension for the [gitlab-ci-ls](https://github.com/alesbrelih/gitlab-ci-ls) languages server.

To use this extension please install the binary. Otherwise it won't work.
Furtheremore this extension only works when the .gitlab-ci-yml is detected as YAML file.

## ShellCheck

`gitlab-ci-ls` lints `script`, `before_script` and `after_script` with [ShellCheck](https://www.shellcheck.net) and shows the findings in the YAML file.
This needs a `gitlab-ci-ls` release newer than 1.4.0.

The extension uses the `shellcheck` binary from your `PATH`. If there is none, it downloads the latest ShellCheck release.

ShellCheck is configured through the initialization options of the language server in your Zed settings:

```json
{
  "lsp": {
    "gitlab-ci": {
      "initialization_options": {
        "options": {
          "shellcheck": {
            "enabled": true,
            "path": "/usr/local/bin/shellcheck",
            "shell": "bash",
            "severity": "style",
            "exclude": ["SC2086"]
          }
        }
      }
    }
  }
}
```

All keys are optional. Set `enabled` to `false` to turn ShellCheck off; a `path` you set is used as is and nothing gets downloaded.
