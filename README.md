# zed-gitlab-ci-ls

GitLab CI language support for Zed, using the
[gitlab-ci-ls](https://github.com/alesbrelih/gitlab-ci-ls) language server.

## Features

- Detects `.gitlab-ci.yml` and `.gitlab-ci.yaml`, as well as files ending in
  `.gitlab-ci.yml` or `.gitlab-ci.yaml` (e.g. `build.gitlab-ci.yml`), as **Gitlab-CI**.
- Retains YAML highlighting, bracket matching, comments, indentation, and outline.
- Injects Bash highlighting into `script`, `before_script`, `after_script`, and
  `pre_get_sources_script`, including global, default, hidden-job, and hook entries.
- Recognizes single commands, block lists, flow lists, and literal (`|`) or folded
  (`>`) multiline scalars, including chomping and indentation indicators.

```yaml
default:
  before_script:
    - export BUILD_DIR=build

build:
  script:
    - echo "Building in $BUILD_DIR"
    - |-
      if test -d "$BUILD_DIR"; then
        echo "Reusing build directory"
      fi
  after_script: echo "Finished"
```

### Highlighting limitations

Zed injects the raw YAML scalar text. Its YAML grammar does not expose separate
content nodes for quoted or block scalars, and Zed does not implement the
`#offset!` directive used by some other editors. Consequently, YAML quotes and
block headers are passed to Bash too. **YAML-quoted commands can appear as shell
strings rather than highlighting their inner commands**, and block headers can
produce parser errors even though the following shell code is highlighted.
Prefer plain commands or literal blocks for the best results.

Folded lines and YAML escapes are not decoded before highlighting. Each command
is parsed independently; shell constructs split across separate list entries
are not combined. Nested command arrays are not traversed, and aliases and
`!reference` values are not expanded. Bash highlighting also does not model
PowerShell or Windows batch runners.

## Language server

Install the `gitlab-ci-ls` binary and make sure it is available on Zed's `PATH`.
Syntax highlighting works without the binary; language-server features require it.

The server now attaches to **Gitlab-CI**, not to every YAML file, and still receives
`yaml` as the LSP language ID. If you previously configured this extension under
`languages.YAML`, move those settings to `languages.Gitlab-CI`. Remove any custom
file association forcing `.gitlab-ci.yml` to YAML.

For included CI files with other names, select **Gitlab-CI** manually or add a
file association in Zed's settings, for example:

```json
{
  "file_types": {
    "Gitlab-CI": [".gitlab/ci/**/*.yml"]
  }
}
```

## Shell scripts and schema completion (`gitlab-ci-bash-ls`)

`gitlab-ci-ls` completes values such as stages, `extends`, and `needs`, but not
keys. The optional `gitlab-ci-bash-ls` server fills the gaps. Like `helm-ls`, it
is a proxy in front of two existing language servers:

- [bash-language-server](https://github.com/bash-lsp/bash-language-server) for
  the scripts inside a CI file: ShellCheck diagnostics, hover, and completion.
  Every script is opened as a virtual `.sh` document, and all positions are
  mapped back to the YAML file.
- [yaml-language-server](https://github.com/redhat-developer/yaml-language-server)
  for the rest of the file, with GitLab's official CI JSON schema: key completion
  (for example `stage`, `script`, or `rules` inside a job), hover, and
  validation. Zed attaches its own YAML server only to the YAML language, not to
  **Gitlab-CI**, so this replaces it for CI files.

```sh
npm install -g bash-language-server yaml-language-server
# or: brew install bash-language-server yaml-language-server
# plus ShellCheck, e.g. brew install shellcheck / apt install shellcheck
cargo install --path gitlab-ci-bash-ls   # from a checkout of this repository
```

Each backend is optional; if one is missing, the proxy shows a warning and
provides the features of the other.

The extension finds all binaries on Zed's `PATH`. To use other locations:

```json
{
  "lsp": {
    "gitlab-ci-bash-ls": {
      "binary": {
        "path": "/path/to/gitlab-ci-bash-ls",
        "env": {
          "BASH_LANGUAGE_SERVER_PATH": "/path/to/bash-language-server",
          "YAML_LANGUAGE_SERVER_PATH": "/path/to/yaml-language-server"
        }
      }
    }
  }
}
```

If you do not want it, disable it for the language:

```json
{
  "languages": {
    "Gitlab-CI": { "language_servers": ["gitlab-ci", "!gitlab-ci-bash-ls"] }
  }
}
```

Which keys are checked matches the highlighting above. A job's `before_script`
and `script` form one document, because GitLab runs them in the same shell;
`after_script` and `hooks:pre_get_sources_script` are separate documents.
Nested command lists are flattened; `!reference` entries and aliases are skipped.

Settings go under `lsp.gitlab-ci-bash-ls.settings`:

```json
{
  "lsp": {
    "gitlab-ci-bash-ls": {
      "settings": {
        "shell": "sh",
        "bashIde": {
          "shellcheckArguments": ["--exclude=SC2154,SC2034"]
        }
      }
    }
  }
}
```

- `shell` sets the ShellCheck dialect (`sh`, `bash`, `dash`, `ksh`, ...). The
  default is `bash`; set `sh` for Alpine/BusyBox images.
- `bashIde` is passed to bash-language-server as its configuration. The proxy
  defaults to `backgroundAnalysisMaxFiles: 0` and
  `shellcheckArguments: ["--exclude=SC2154"]`, because CI variables are defined
  by GitLab rather than in the script. Setting `shellcheckArguments` replaces
  that default.
- `yaml` is passed to yaml-language-server as its `yaml` configuration and wins
  over everything else. By default, the proxy takes your settings for Zed's own
  YAML server (`lsp.yaml-language-server.settings.yaml`, for example `format`),
  pins the schema to GitLab's
  [`ci.json`](https://gitlab.com/gitlab-org/gitlab/-/raw/master/app/assets/javascripts/editor/schema/ci.json)
  (downloaded by yaml-language-server), and registers the `!reference` tag.
  Override `yaml.schemas` to use a local or self-hosted schema.

Limitations: quick fixes, formatting, rename and go-to-definition are not
forwarded yet. Positions inside quoted or folded YAML scalars are mapped char by
char and can be approximate for unusual escapes or line folding. Completion edits
are inserted as-is, so inside a YAML-quoted command you may need to escape them.

## Testing

The query tests use the same YAML grammar revision as `extension.toml`. With
Python 3.11+, Git, and a C compiler installed:

```sh
python3 -m venv target/query-tests
target/query-tests/bin/python -m pip install -r tests/requirements.txt
target/query-tests/bin/python -m unittest discover -s tests
cargo check --locked
cargo test -p gitlab-ci-bash-ls
```

The `gitlab-ci-bash-ls` end-to-end test runs only when `bash-language-server` and
`shellcheck` are installed; otherwise it is skipped.

On Windows, use `target/query-tests/Scripts/python.exe` instead. To verify the
rendered result, install this repository using Zed's **Install Dev Extension**
command and open a `.gitlab-ci.yml` file.
