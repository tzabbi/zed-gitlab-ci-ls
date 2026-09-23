"""Exercise the query captures Zed uses, without unsupported offset directives."""

import unittest
from pathlib import Path

import tomllib
import tree_sitter_bash
import tree_sitter_yaml
from tree_sitter import Language, Parser, Query, QueryCursor

ROOT = Path(__file__).resolve().parents[1]
LANGUAGE_DIR = ROOT / "languages" / "gitlab-ci"
SCRIPT_KEYS = ("script", "before_script", "after_script", "pre_get_sources_script")


class GitlabCiLanguageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.yaml = Language(tree_sitter_yaml.language())
        cls.bash = Language(tree_sitter_bash.language())
        cls.parser = Parser(cls.yaml)
        cls.injections = Query(cls.yaml, (LANGUAGE_DIR / "injections.scm").read_text())

    def contents(self, source):
        tree = self.parser.parse(source.encode())
        self.assertFalse(tree.root_node.has_error, str(tree.root_node))
        result = []
        for pattern, captures in QueryCursor(self.injections).matches(tree.root_node):
            self.assertEqual(
                self.injections.pattern_settings(pattern),
                {"injection.language": "bash"},
            )
            result.extend(node.text.decode() for node in captures["injection.content"])
        return result

    def test_registration(self):
        config = tomllib.loads((LANGUAGE_DIR / "config.toml").read_text())
        manifest = tomllib.loads((ROOT / "extension.toml").read_text())
        self.assertEqual(config["name"], "Gitlab-CI")
        self.assertEqual(config["path_suffixes"], [".gitlab-ci.yml", ".gitlab-ci.yaml"])
        for name in ("gitlab-ci", "gitlab-ci-bash-ls"):
            with self.subTest(server=name):
                server = manifest["language_servers"][name]
                self.assertEqual(server["languages"], [config["name"]])
                self.assertEqual(server["language_ids"][config["name"]], "yaml")
        grammar = manifest["grammars"][config["grammar"]]
        requirements = (ROOT / "tests" / "requirements.txt").read_text()
        self.assertIn(grammar["repository"] + "@" + grammar["rev"], requirements)

    def test_all_queries_compile(self):
        for path in LANGUAGE_DIR.glob("*.scm"):
            with self.subTest(query=path.name):
                Query(self.yaml, path.read_text())

    def test_single_commands_and_quoted_keys(self):
        for key in SCRIPT_KEYS:
            for quoted_key in (key, f"'{key}'", f'"{key}"'):
                for command in ("echo hello", "'echo hello'", '"echo hello"'):
                    with self.subTest(key=quoted_key, command=command):
                        self.assertEqual(
                            self.contents(f"job:\n  {quoted_key}: {command}\n"),
                            [command],
                        )

    def test_block_and_flow_lists(self):
        commands = ["echo plain", "'echo single'", '"echo double"']
        for key in SCRIPT_KEYS:
            with self.subTest(key=key, style="block"):
                items = "".join(f"    - {command}\n" for command in commands)
                self.assertEqual(self.contents(f"job:\n  {key}:\n{items}"), commands)
            with self.subTest(key=key, style="flow"):
                items = ", ".join(commands)
                self.assertEqual(self.contents(f"job:\n  {key}: [{items}]\n"), commands)
            with self.subTest(key=key, style="flow mapping"):
                self.assertEqual(
                    self.contents(f"job: {{{key}: [{items}]}}\n"), commands
                )
                self.assertEqual(
                    self.contents(f"job: {{{key}: echo hello}}\n"), ["echo hello"]
                )

    def test_multiline_commands(self):
        for key in SCRIPT_KEYS:
            for header in ("|", "|-", "|+", "|2", ">", ">-", ">+", ">2-"):
                for sequence in (False, True):
                    with self.subTest(key=key, header=header, sequence=sequence):
                        if sequence:
                            source = f"job:\n  {key}:\n    - {header}\n      echo first\n      echo second\n"
                        else:
                            source = f"job:\n  {key}: {header}\n    echo first\n    echo second\n"
                        captures = self.contents(source)
                        self.assertEqual(len(captures), 1)
                        self.assertTrue(captures[0].startswith(header))
                        self.assertIn("echo first", captures[0])
                        self.assertIn("echo second", captures[0])

    def test_global_default_and_hook_scripts(self):
        source = """before_script: echo global
.default_job:
  script: echo hidden
default:
  before_script:
    - echo default
  after_script: echo cleanup
  hooks:
    pre_get_sources_script:
      - echo hook
job:
  script: echo job
"""
        self.assertCountEqual(
            self.contents(source),
            [
                "echo global",
                "echo hidden",
                "echo default",
                "echo cleanup",
                "echo hook",
                "echo job",
            ],
        )

    def test_anchors_aliases_and_references(self):
        self.assertEqual(
            self.contents(
                "job:\n  script: &command echo hello\n  after_script: *command\n"
            ),
            ["echo hello"],
        )
        self.assertEqual(
            self.contents("job:\n  script: &commands [echo hello, echo world]\n"),
            ["echo hello", "echo world"],
        )
        for key in SCRIPT_KEYS:
            for reference in (
                "!reference [.template, script]",
                "&ref !reference [.template, script]",
            ):
                with self.subTest(key=key, reference=reference):
                    self.assertEqual(self.contents(f"job:\n  {key}: {reference}\n"), [])
                    self.assertEqual(
                        self.contents(
                            f"job:\n  {key}:\n    - {reference}\n    - echo hello\n"
                        ),
                        ["echo hello"],
                    )

    def test_multiline_references_remain_yaml(self):
        for source in (
            "job:\n  script: !reference\n    - .template\n    - script\n",
            "job:\n  script: &ref # comment\n    !reference [.template, script]\n",
            "job:\n  script: !reference &ref\n    - .template\n    - script\n",
        ):
            with self.subTest(source=source):
                self.assertEqual(self.contents(source), [])

    def test_script_named_metadata_remains_yaml(self):
        for key in SCRIPT_KEYS:
            for variables in ("variables", "'variables'", '"variables"'):
                for value in (
                    "echo metadata",
                    "[echo metadata]",
                    "|\n    echo metadata",
                ):
                    source = (
                        f"{variables}:\n  {key}: {value}\njob:\n  script: echo actual\n"
                    )
                    with self.subTest(source=source):
                        self.assertEqual(self.contents(source), ["echo actual"])
            source = (
                f"job:\n  variables:\n    {key}: echo metadata\n  script: echo actual\n"
            )
            self.assertEqual(self.contents(source), ["echo actual"])

    def test_non_script_values_remain_yaml(self):
        source = """# script: echo not a command
variables:
  MESSAGE: echo not a command
  MULTILINE: |
    echo not a command
job:
  image: alpine
  description: echo not a command
  script_name: echo not a command
  rules:
    - if: '$CI_COMMIT_BRANCH == "main"'
  tags: [echo not a command]
  script:
    - echo real command
  artifacts:
    paths: [build]
"""
        self.assertEqual(self.contents(source), ["echo real command"])

    def test_plain_and_block_commands_have_shell_tokens(self):
        source = """job:
  before_script: export BUILD=ready
  script: |-
    if test -n "$BUILD"; then
      echo "$BUILD"
    fi
  after_script:
    - echo "$BUILD"
"""
        query = Query(
            self.bash, '(command_name) @command (variable_name) @variable "if" @keyword'
        )
        parser = Parser(self.bash)
        tokens = []
        for content in self.contents(source):
            # Parse the exact node text, just as Zed does, including YAML headers.
            tree = parser.parse(content.encode())
            captures = QueryCursor(query).captures(tree.root_node)
            tokens.extend(
                node.text.decode() for nodes in captures.values() for node in nodes
            )
        self.assertIn("echo", tokens)
        self.assertIn("BUILD", tokens)
        self.assertIn("if", tokens)


if __name__ == "__main__":
    unittest.main()
