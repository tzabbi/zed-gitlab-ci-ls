# zed-gitlab-ci-ls

GitLab CI language support for Zed, using the
[gitlab-ci-ls](https://github.com/alesbrelih/gitlab-ci-ls) language server.

Upgrading from 1.x? Read the [migration guide](#migrating-from-1x-to-20).
See [CHANGELOG.md](CHANGELOG.md) for the changes in 2.0.0.

## Installation

Install via Zed Extension store: [https://zed.dev/extensions/gitlab-ci-ls].

You have to install `bash-language-server` and `yaml-language-server` by your own.

## Features

- Detects `.gitlab-ci.yml` and `.gitlab-ci.yaml`, as well as files ending in
  `.gitlab-ci.yml` or `.gitlab-ci.yaml` (e.g. `build.gitlab-ci.yml`), as **Gitlab-CI**.
- Retains YAML highlighting, bracket matching, comments, indentation, and outline.
- Injects Bash highlighting into `script`, `before_script`, `after_script`, and
  `pre_get_sources_script`, including global, default, hidden-job, and hook entries.
- Recognizes single commands, block lists, flow lists, and literal (`|`) or folded
  (`>`) multiline scalars, including chomping and indentation indicators.
- Automatically installs the optional `gitlab-ci-bash-ls` proxy from a matching
  GitHub release on supported platforms, unless a configured binary or one on
  Zed's `PATH` takes precedence. See [proxy installation](#proxy-installation).

### Indentation and script completion

Enter after `script:`, `- |`, or `- >` increases indentation by one level
(two spaces by default). Block modifiers such as `|-`, `>+`, and `|2-`, and
trailing header comments, are recognized too. Empty block scalars stay in YAML
until they contain a command, so Bash's indentation settings do not take over
while you create the block.

When confirming `script`, `before_script`, or `after_script` completion with
Enter or Tab, `gitlab-ci-bash-ls` inserts a key and an indented first list item:

```yaml
test:
  after_script:
    -  # cursor starts here
```

The completion also handles an unindented partial key **immediately after an
empty job header** (for example `test:\naft`). It does not reparent global hooks
after a completed job, blank separator, or comment, or rewrite existing values.
Existing job indentation wins; for an empty job the proxy uses `yaml.indentation`
if configured, otherwise the file's indentation or two spaces. `default` offers
only `before_script` and `after_script`, not `script`.

Once a block contains shell code, Zed can use **Shell Script** settings inside
that injection, including when editing its header. For consistent two-space
indentation there too, set `languages."Shell Script".tab_size` to `2` in Zed's
project settings. This also affects standalone shell files in that project.

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

## Migrating from 1.x to 2.0

Version 2.0 introduces a dedicated **Gitlab-CI** language. This changes which
editor settings and language servers apply to CI files. The extension does not
rewrite existing pipeline files, and upgrading alone does not change their
execution in GitLab.

Merge the examples below into your existing user or project settings; do not
replace your entire configuration.

### 1. Check file associations

Open a CI file and confirm that Zed's language selector shows **Gitlab-CI**.
`.gitlab-ci.yml`, `.gitlab-ci.yaml`, and names such as `build.gitlab-ci.yml` are
detected automatically.

Remove CI-specific patterns from custom `file_types.YAML` associations that force
these files to remain YAML. Do not remove YAML associations for unrelated files.
For included CI files with other names, add an explicit association, for example:

```json
{
  "file_types": {
    "Gitlab-CI": [".gitlab/ci/**/*.yml", ".gitlab/ci/**/*.yaml"]
  }
}
```

Scope these patterns to CI configuration, not all YAML files. Keeping a CI file
as **YAML** retains the ordinary YAML tooling, but the extension's `gitlab-ci`
server no longer attaches to it automatically.

### 2. Copy relevant language settings

CI-specific editor settings previously placed under `languages.YAML` belong under
`languages."Gitlab-CI"` now. Copy settings such as indentation, formatting choices,
and language-server selection; retain the original YAML settings if other files
still need them.

For example, to use two-space indentation:

```json
{
  "languages": {
    "Gitlab-CI": {
      "tab_size": 2
    }
  }
}
```

The server IDs remain `gitlab-ci` for the original server and
`gitlab-ci-bash-ls` for the new proxy. Do not rename server-specific `lsp` settings
to `Gitlab-CI`; that is the language name, not a server ID. The LSP language ID
sent to the servers remains `yaml`.

### 3. Use automatic proxy installation or explicitly disable it

Keep the existing `gitlab-ci-ls` executable on Zed's `PATH`. It still supplies
GitLab-specific completion for values such as stages, `extends`, and `needs`.

The new `gitlab-ci-bash-ls` proxy is optional, but registered by default. Zed
installs it automatically on supported platforms when no explicit
`lsp.gitlab-ci-bash-ls.binary.path` or executable on Zed's `PATH` is available.
Automatic installation requires published release assets matching the extension
version; the first download requires GitHub access. An already completed cache
for that exact version and platform works offline. See
[proxy installation](#proxy-installation) for supported platforms and manual options.

**Full YAML/Bash support:** let Zed install the proxy and install the desired
backend tools yourself:

```sh
npm install -g bash-language-server yaml-language-server
# Install ShellCheck with your system package manager, e.g. brew install shellcheck.
```

If you already restrict `language_servers` for the language, include both servers:

```json
{
  "languages": {
    "Gitlab-CI": {
      "language_servers": ["gitlab-ci", "gitlab-ci-bash-ls"]
    }
  }
}
```

Preserve any additional servers you intentionally use. The proxy can operate
with just one backend; missing backends produce a warning. Without ShellCheck,
its diagnostics are unavailable. See [proxy setup](#shell-scripts-and-schema-completion-gitlab-ci-bash-ls)
for binary path overrides and settings.

**Original GitLab server only:** explicitly disable the proxy:

```json
{
  "languages": {
    "Gitlab-CI": {
      "language_servers": ["gitlab-ci", "!gitlab-ci-bash-ls"]
    }
  }
}
```

This keeps syntax highlighting and the original server without requiring the
proxy. It does **not** restore Zed's normal YAML server for this language: the
proxy's schema completion/validation, script scaffolds, and Bash/ShellCheck
features will be unavailable.

### 4. Review schemas and formatting

Zed's normal YAML server no longer automatically attaches to these files. The
proxy inherits `lsp.yaml-language-server.settings.yaml`, but overrides schema
selection with GitLab's CI schema and disables SchemaStore by default. If you
used a custom CI schema, configure it explicitly under
`lsp.gitlab-ci-bash-ls.settings.yaml.schemas`. Overrides there take precedence.

The default GitLab schema is downloaded by `yaml-language-server`; offline or
restricted-network setups should use a local schema. See the
[proxy configuration](#shell-scripts-and-schema-completion-gitlab-ci-bash-ls).

The proxy forwards completion, hover, and diagnostics, **not** formatting,
code actions/quick fixes, rename, or go-to-definition. If your old setup used the
YAML server as its formatter, migrate to a suitable formatter such as Prettier
and copy the relevant formatter configuration to **Gitlab-CI**. The language
still declares Prettier's YAML parser. Inheriting `yaml.format` settings does not
make LSP formatting available through the proxy.

### 5. Review editing behavior

- Confirming a script-key completion with Enter or Tab now inserts an indented
  key and first list item. Directly beneath an empty job header, an unindented
  partial key can become a job property rather than a global hook. To deliberately
  create a global hook, use a separate root-level context (for example after a
  blank separator); prefer `default.before_script` or `default.after_script` for
  shared hooks. Existing files are not reformatted automatically.
- Enter after `- |` or `- >` now requests another indentation level, two spaces
  by default. Empty blocks remain in YAML while being created.
- Inside a nonempty shell block, Zed can use **Shell Script** indentation settings.
  If you also want two spaces there, optionally add the following project setting.
  It affects standalone shell files in that project too:

```json
{
  "languages": {
    "Shell Script": {
      "tab_size": 2
    }
  }
}
```

The known `>` highlighting issue is not a migration failure: the raw header can
be parsed as Bash output redirection. Literal `|` blocks generally work better
for multiline shell control flow, but `|` preserves line breaks while `>` folds
many of them. Do not change existing blocks without considering that semantic
difference. See [highlighting limitations](#highlighting-limitations).

### 6. Reload and verify

After upgrading, restart the language servers or restart Zed. Existing proxy
binaries on Zed's `PATH` take precedence and are not updated automatically.
For a dev extension, rebuild it in Zed after updating the checkout and separately
reinstall or rebuild the proxy when its source changes; rebuilding the extension
does not build the proxy or publish release assets. See
[manual installation and development](#manual-installation-and-development).

Check the following:

- The file is recognized as **Gitlab-CI** and the selected servers start without errors.
- With the proxy's YAML backend enabled, job-key completion works.
- With Bash and ShellCheck installed, commands receive Bash features and diagnostics.
- Accepting a script completion and pressing Enter after an empty `- |` or `- >`
  produces the intended indentation.
- Your chosen formatter still works for CI files.

## Language server

Install the original `gitlab-ci-ls` binary manually and make sure it is available
on Zed's `PATH`; the proxy installer does not install it. Syntax highlighting
works without this binary; the original server's language features require it.

The server attaches to **Gitlab-CI**, not to every YAML file, and still receives
`yaml` as the LSP language ID. For existing 1.x setups, follow the
[migration guide](#migrating-from-1x-to-20).

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

### Proxy installation

Zed resolves the proxy in this order:

1. An explicit `lsp.gitlab-ci-bash-ls.binary.path`.
2. A `gitlab-ci-bash-ls` executable on Zed's `PATH`.
3. A completed cached download for the **exact extension version and platform**.
4. A download from the matching GitHub release in
   [`tzabbi/zed-gitlab-ci-ls`](https://github.com/tzabbi/zed-gitlab-ci-ls/releases).

The release tag is `v` plus the extension/root `Cargo.toml` version: currently
`v2.0.0`, not the proxy crate's own version. There is no lookup for `latest`, no
independent automatic upgrade to a newer proxy release, and no fallback to a
cached binary for another version or platform. Explicit paths and `PATH` binaries
take precedence over the managed cache and are not updated automatically.

The installer downloads a native **raw executable** (not an archive) into Zed's
extension work directory. It requires neither administrator privileges nor
Rust/Cargo. Supported platforms and expected release asset names are:

| Platform             | Asset                                          |
| -------------------- | ---------------------------------------------- |
| Linux x86_64 (musl)  | `gitlab-ci-bash-ls-x86_64-unknown-linux-musl`  |
| Linux aarch64 (musl) | `gitlab-ci-bash-ls-aarch64-unknown-linux-musl` |
| macOS Intel          | `gitlab-ci-bash-ls-x86_64-apple-darwin`        |
| macOS Apple Silicon  | `gitlab-ci-bash-ls-aarch64-apple-darwin`       |
| Windows x86_64       | `gitlab-ci-bash-ls-x86_64-pc-windows-msvc.exe` |

Automatic installation works only after the matching release and assets have
been published. The first automatic download contacts `api.github.com`,
`github.com`, and GitHub's release-asset hosts (including redirect destinations);
allow these through any firewall or proxy. A completed compatible cache is used
**offline without a release lookup**. An extension upgrade needs its own matching
cached binary or a new download. Backend tools and remote YAML schemas have
separate installation and network requirements.

On an unsupported platform, a network failure, or a missing release/asset, the
installer reports an actionable error rather than using a mismatched cache.
Use a manual Cargo installation/build or set `lsp.gitlab-ci-bash-ls.binary.path`
as described below, or disable the optional proxy.

### Manual installation and development

From a checkout of this repository, with Rust/Cargo installed:

```sh
cargo install --locked --path gitlab-ci-bash-ls --force
```

Ensure Cargo's binary directory is on **Zed's** `PATH`, or set
`lsp.gitlab-ci-bash-ls.binary.path` to the installed executable. Alternatively,
run `cargo build --locked --release -p gitlab-ci-bash-ls` and point that setting
to the absolute path of `target/release/gitlab-ci-bash-ls` (`.exe` on Windows).

Editing source or rebuilding a dev extension neither builds the native proxy
nor publishes GitHub release assets. A dev extension still requests the release
tag matching its extension/root Cargo version unless a local binary takes
precedence. After proxy source changes, rerun the installation command above or
rebuild the binary selected by `binary.path`, then restart the language servers.
An existing local `PATH` version will continue to win until you update or remove it.

### Backend tools and configuration

`bash-language-server`, `yaml-language-server`, and ShellCheck remain externally
installed; the automatic installer installs only the proxy, not these tools or
the original `gitlab-ci-ls` server.

```sh
npm install -g bash-language-server yaml-language-server
# or: brew install bash-language-server yaml-language-server
# plus ShellCheck, e.g. brew install shellcheck / apt install shellcheck
```

Each backend is optional; if one is missing, the proxy shows a warning and
provides the features of the other. Without ShellCheck, its diagnostics are
unavailable.

Backend executables are found on Zed's `PATH`. To override the proxy and backend
locations (omit `binary.path` to keep automatic proxy installation):

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
forwarded yet. Source ranges include complete YAML escapes and doubled quotes;
positions inside an escape are not treated as editable decoded-character boundaries.
Unusual scalar folding can still produce approximate source positions. Shell
completion replacement text is inserted as-is, so inside a YAML-quoted command
you may need to escape newly inserted quotes or backslashes.

## Testing

The query tests use the same YAML grammar revision as `extension.toml`. With
Python 3.11+, Git, and a C compiler installed:

```sh
python3 -m venv target/query-tests
target/query-tests/bin/python -m pip install -r tests/requirements.txt
target/query-tests/bin/python -m unittest discover -s tests
cargo check --locked --workspace
cargo test --locked --workspace
```

The extension's installer tests use a fake download host and temporary directories
(no network). They check binary precedence, version/platform selection, offline
cache reuse, interrupted downloads, permissions failures, and actionable errors.

The `gitlab-ci-bash-ls` end-to-end tests run only when `bash-language-server`,
`yaml-language-server`, and `shellcheck` are installed; otherwise they are skipped.
They exercise snippet and plain-text completion, indentation, resolve, and escaped
source ranges against real backends using an offline schema. The Python tests
check indentation regexes and syntax captures, not Zed's actual cursor movement.

After changing the proxy, reinstall it with `cargo install --locked --path
gitlab-ci-bash-ls --force` (or rebuild the executable selected by `binary.path`),
rebuild the dev extension in Zed, and restart its language
servers. Manually check Enter/Tab completion beneath an empty job, Enter after
`- |` and `- >` (also before existing blank lines), and shell highlighting after
adding the first command.

On Windows, use `target/query-tests/Scripts/python.exe` instead. To verify the
rendered result, install this repository using Zed's **Install Dev Extension**
command and open a `.gitlab-ci.yml` file.

## Maintainer release checklist

Automatic installation depends on published assets, not just an extension build.
The release workflow in `.github/workflows/release.yaml` is responsible for
building the five raw assets listed above and publishing them before the Zed
extension registry action runs.

1. Keep `extension.toml` and the root `Cargo.toml` versions in agreement, and use
   the matching `v<version>` release tag (currently `v2.0.0`). The proxy crate's
   own version does not select the download tag.
2. Verify that the workflow builds all five native binaries and uploads them as
   raw assets named `gitlab-ci-bash-ls-<Rust triple>[.exe]`, exactly as listed above.
3. Configure the `COMMITTER_TOKEN` repository secret with the permissions required
   by `huacnlee/zed-extension-action` to update `tzabbi/extensions`. Binary publication
   uses the workflow's `GITHUB_TOKEN` with `contents: write`.
4. Confirm that the matching GitHub release and all five assets are published
   before the registry action publishes the extension. A source tag, checkout,
   or rebuilt dev extension alone is not sufficient.

The workflow keeps new releases in draft until all uploads are verified. Retrying
can replace incomplete draft assets, but never overwrites published assets. For
an already-published release, it verifies that the assets match the build before
continuing to the registry action. If rebuilt bytes differ, use a new version/tag
rather than changing binaries that users may already have cached.
