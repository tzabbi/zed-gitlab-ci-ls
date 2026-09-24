# Changelog

## 2.0.0

This major update introduces a dedicated GitLab CI language and an optional
YAML/Bash language-server proxy. Existing Zed configurations may require changes.
See the [migration guide](README.md#migrating-from-1x-to-20) before upgrading.

### Breaking changes

- GitLab CI files are now recognized as **Gitlab-CI**, rather than ordinary YAML.
  The `gitlab-ci` language server attaches only to this new language. CI files
  explicitly associated with YAML, or included files with other names, need their
  file associations updated to retain GitLab-specific language-server features.
- Settings under `languages.YAML` no longer automatically apply to GitLab CI
  files. Copy relevant editor settings to `languages."Gitlab-CI"`; retain YAML
  settings needed by other files.
- Zed's normal YAML language server does not automatically attach to **Gitlab-CI**.
  Schema-based completion, hover, and validation are provided through the new
  `gitlab-ci-bash-ls` proxy instead. The proxy does not forward formatting, code
  actions/quick fixes, rename, or go-to-definition, so workflows depending on
  those YAML-server features need review. Prettier's YAML parser remains configured.
- The optional `gitlab-ci-bash-ls` server is registered by default and now installs
  automatically on supported platforms when no configured or `PATH` binary is
  available. Install the desired backend tools separately, or explicitly disable
  the proxy. The original `gitlab-ci-ls` binary remains manually installed and
  independently usable.

### Added

- Detection of `.gitlab-ci.yml`, `.gitlab-ci.yaml`, and filenames ending in
  `.gitlab-ci.yml` or `.gitlab-ci.yaml`.
- YAML-based highlighting, brackets, comments, outline, and indentation for the
  dedicated language.
- Bash highlighting in `script`, `before_script`, `after_script`, and
  `pre_get_sources_script`, including supported global, default, hidden-job, and
  hook contexts.
- A `gitlab-ci-bash-ls` proxy for Bash hover/completion and ShellCheck diagnostics,
  plus YAML key completion, hover, and validation using GitLab's official CI schema.
- Automatic proxy installation from `tzabbi/zed-gitlab-ci-ls` GitHub releases, using
  `v` plus the extension/root Cargo version (currently `v2.0.0`). Resolution order
  is explicit `lsp.gitlab-ci-bash-ls.binary.path`, Zed's `PATH`, a completed cache
  for the exact extension version and platform, then the matching release download.
- A release workflow that builds all five platform assets, verifies uploads before
  publishing, and updates the Zed registry afterward. Release retries never overwrite
  published binaries.
- Native raw proxy binaries downloaded into Zed's extension work directory, with
  no administrator privileges or Rust toolchain required. Supported targets are
  Linux x86_64/aarch64 (musl), macOS Intel/Apple Silicon, and Windows x86_64.
- Actionable installer errors for unsupported platforms, network failures, and
  missing releases/assets, with manual Cargo install/build or `binary.path`
  alternatives; the proxy can also be disabled.
- Virtual shell documents with positions mapped back to YAML. A job's
  `before_script` and `script` share a document; cleanup and hook scripts are separate.
- Backend path overrides, ShellCheck dialect/options, `!reference` tag support,
  inherited YAML-server settings, and local/self-hosted schema configuration.
- Script-key completion scaffolds with an indented first list item, supporting
  both snippet-capable and plain-text clients.
- Offline installer tests for binary precedence, compatible cache reuse, platform
  selection, failed/partial downloads, and missing releases/assets.
- Query, source-map, completion, and real-backend integration tests. Integration
  tests use an offline schema and run when the required backend tools are installed.

### Changed

- Confirming `script`, `before_script`, or `after_script` completion immediately
  beneath an empty job header can indent an unindented partial key into that job.
  This intentionally changes the insertion context compared with a global key.
  Existing job indentation is respected, duplicate keys are avoided, and global
  hooks after completed jobs or blank/comment separators are not reparented.
- Empty block scalars remain in YAML until they contain non-whitespace body text,
  preventing Bash indentation settings from taking over while creating a block.

### Fixed

- Indentation rules now recognize sequence block headers (`- |`, `- >`), mapping
  block headers with chomping/indentation indicators, and trailing header comments.
- Script diagnostics are cleared when script text changes, instead of immediately
  remapping cached diagnostics against the new text.
- Source ranges preserve complete YAML escape sequences and doubled single quotes,
  including completion endpoints. Folded continuation alignment no longer mistakes
  indentation for an escaped space. Positions inside an escape are not treated as
  editable decoded-character boundaries.

### Known limitations

- Automatic proxy installation requires matching published release assets. It
  does not use `latest`, upgrade independently to newer proxy releases, or fall
  back to a cache for a different version/platform. A completed compatible cache
  works offline without any release lookup; the first download needs access to
  `api.github.com`, `github.com`, and GitHub's release-asset hosts. An extension
  upgrade requires its own matching cache or download.
- Configured proxy binaries and existing versions on Zed's `PATH` take precedence
  and are not updated automatically. Source edits or a dev-extension rebuild do
  not build the proxy or publish assets. For local changes, use
  `cargo install --locked --path gitlab-ci-bash-ls --force` or rebuild the executable
  selected by `lsp.gitlab-ci-bash-ls.binary.path`.
- `bash-language-server`, `yaml-language-server`, ShellCheck, and the original
  `gitlab-ci-ls` still require external installation. Offline YAML schema use also
  requires a local schema; the proxy cache does not cache backend tools or schemas.
- Highlighting receives raw YAML scalars, including quotes and block headers.
  In a folded `>` block, Bash can interpret the header as output redirection and
  misclassify a following `if` as a string. Prefer literal `|` blocks for multiline
  shell control flow, keeping YAML's different folding semantics in mind.
- Nonempty shell injections can use Zed's **Shell Script** indentation settings,
  including while editing their headers.
- Highlighting does not combine separate command entries or expand aliases and
  `!reference` values. The proxy also skips alias/reference expansion; unusual
  scalar folding can still yield approximate source positions.
- Shell completion replacement text is not automatically YAML-escaped. New quotes
  or backslashes inserted inside a YAML-quoted command may need escaping.

Updating the extension alone does not rewrite existing CI files or change how
GitLab executes them. Accepting completions is an edit and can change their content.
