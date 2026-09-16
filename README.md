# zed-gitlab-ci-ls

GitLab CI language support for Zed, using the
[gitlab-ci-ls](https://github.com/alesbrelih/gitlab-ci-ls) language server.

## Features

- Detects `.gitlab-ci.yml` and `.gitlab-ci.yaml` as **Gitlab-CI**.
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

## Testing

The query tests use the same YAML grammar revision as `extension.toml`. With
Python 3.11+, Git, and a C compiler installed:

```sh
python3 -m venv target/query-tests
target/query-tests/bin/python -m pip install -r tests/requirements.txt
target/query-tests/bin/python -m unittest discover -s tests
cargo check --locked
```

On Windows, use `target/query-tests/Scripts/python.exe` instead. To verify the
rendered result, install this repository using Zed's **Install Dev Extension**
command and open a `.gitlab-ci.yml` file.
