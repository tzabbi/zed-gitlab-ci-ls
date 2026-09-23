//! Finds the shell scripts in a GitLab CI file and records where each script
//! character came from, so positions can be translated in both directions.

use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

/// Zero-based position in the YAML file; `col` counts Unicode scalar values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    fn next(self) -> Self {
        Pos {
            line: self.line,
            col: self.col + 1,
        }
    }
}

/// Root keys that configure the pipeline instead of naming a job.
/// Kept in sync with the job-name exclusions in `languages/gitlab-ci/injections.scm`.
const RESERVED_KEYS: &[&str] = &[
    "after_script",
    "before_script",
    "cache",
    "hooks",
    "image",
    "include",
    "nil",
    "pre_get_sources_script",
    "script",
    "services",
    "spec",
    "stages",
    "true",
    "false",
    "types",
    "variables",
    "workflow",
];

/// The YAML text split into lines of chars, used to convert between LSP
/// UTF-16 columns and char columns.
pub struct Source {
    lines: Vec<Vec<char>>,
}

impl Source {
    fn new(text: &str) -> Self {
        let lines = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).chars().collect())
            .collect();
        Source { lines }
    }

    fn line(&self, line: usize) -> &[char] {
        self.lines.get(line).map_or(&[], Vec::as_slice)
    }

    /// Returns the UTF-16 column of `pos`, as used by LSP positions.
    pub fn utf16_col(&self, pos: Pos) -> usize {
        let line = self.line(pos.line);
        let within: usize = line.iter().take(pos.col).map(|c| c.len_utf16()).sum();
        within + pos.col.saturating_sub(line.len())
    }

    /// Converts an LSP position (UTF-16 column) to a char-based position.
    pub fn pos(&self, line: usize, utf16_col: usize) -> Pos {
        let mut units = 0;
        let mut col = 0;
        for c in self.line(line) {
            if units >= utf16_col {
                break;
            }
            units += c.len_utf16();
            col += 1;
        }
        Pos { line, col }
    }
}

/// A virtual shell document assembled from one or more YAML script entries.
#[derive(Debug)]
pub struct Script {
    pub name: String,
    pub text: String,
    chars: Vec<char>,
    /// Source position of every char in `chars`; `None` for synthetic text.
    map: Vec<Option<Pos>>,
    line_starts: Vec<usize>,
}

impl Script {
    fn new(name: String, chars: Vec<char>, map: Vec<Option<Pos>>) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            chars
                .iter()
                .enumerate()
                .filter(|(_, c)| **c == '\n')
                .map(|(i, _)| i + 1),
        );
        Script {
            name,
            text: chars.iter().collect(),
            chars,
            map,
            line_starts,
        }
    }

    /// Inserts a synthetic first line, e.g. a ShellCheck directive. Positions
    /// inside it do not map back to the YAML file.
    pub fn prepend_line(self, line: &str) -> Self {
        let mut chars: Vec<char> = line.chars().chain(['\n']).collect();
        let mut map = vec![None; chars.len()];
        chars.extend(self.chars);
        map.extend(self.map);
        Script::new(self.name, chars, map)
    }

    /// Converts an LSP position in the virtual document to a char offset.
    pub fn offset(&self, line: usize, utf16_col: usize) -> usize {
        let Some(&start) = self.line_starts.get(line) else {
            return self.chars.len();
        };
        let mut units = 0;
        let mut offset = start;
        while offset < self.chars.len() && self.chars[offset] != '\n' && units < utf16_col {
            units += self.chars[offset].len_utf16();
            offset += 1;
        }
        offset
    }

    /// Converts a char offset to an LSP position (line, UTF-16 column) in the virtual document.
    pub fn position(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.chars.len());
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let col = self.chars[self.line_starts[line]..offset]
            .iter()
            .map(|c| c.len_utf16())
            .sum();
        (line, col)
    }

    fn to_source(&self, offset: usize) -> Option<Pos> {
        match self.map.get(offset) {
            Some(pos) => *pos,
            None => self.map.iter().rev().flatten().next().map(|pos| pos.next()),
        }
    }

    /// Maps the virtual char range `start..end` to a YAML range.
    pub fn source_range(&self, start: usize, end: usize) -> Option<(Pos, Pos)> {
        let from = self.to_source(start)?;
        let to = if end > start {
            self.to_source(end - 1).map_or(from, Pos::next)
        } else {
            from
        };
        Some((from, to.max(from)))
    }

    /// Returns the virtual char offset for a YAML position inside this script.
    /// A position just after the last char of a line also matches, so that
    /// completion at the end of a command works.
    pub fn offset_of_source(&self, pos: Pos) -> Option<usize> {
        let find = |target: Pos| self.map.iter().position(|p| *p == Some(target));
        find(pos).or_else(|| {
            let before = Pos {
                line: pos.line,
                col: pos.col.checked_sub(1)?,
            };
            find(before).map(|i| i + 1)
        })
    }
}

/// Parses `text` and returns the source lines plus every script it contains.
/// Invalid YAML still yields the scripts parsed before the error.
pub fn parse(text: &str) -> (Source, Vec<Script>) {
    let source = Source::new(text);
    let mut builder = Builder::default();
    // Errors are expected while the user is typing; keep what was parsed so far.
    let _ = Parser::new_from_str(text).load(&mut builder, true);
    while !builder.stack.is_empty() {
        builder.close();
    }

    let mut scripts = Vec::new();
    for document in &builder.documents {
        let Node::Map(entries) = document else {
            continue;
        };
        for (key, value) in entries {
            let Some(key) = key.as_str() else {
                continue;
            };
            if key == "before_script" || key == "after_script" {
                add_script(&mut scripts, &source, key.to_owned(), &[Some(value)]);
                continue;
            }
            if RESERVED_KEYS.contains(&key) {
                continue;
            }
            // GitLab runs before_script and script in the same shell.
            add_script(
                &mut scripts,
                &source,
                format!("{key}/script"),
                &[value.get("before_script"), value.get("script")],
            );
            add_script(
                &mut scripts,
                &source,
                format!("{key}/after_script"),
                &[value.get("after_script")],
            );
            add_script(
                &mut scripts,
                &source,
                format!("{key}/pre_get_sources_script"),
                &[value.get("pre_get_sources_script")],
            );
            add_script(
                &mut scripts,
                &source,
                format!("{key}/hooks/pre_get_sources_script"),
                &[value
                    .get("hooks")
                    .and_then(|hooks| hooks.get("pre_get_sources_script"))],
            );
        }
    }
    (source, scripts)
}

fn add_script(scripts: &mut Vec<Script>, source: &Source, name: String, values: &[Option<&Node>]) {
    let mut items = Vec::new();
    for value in values.iter().flatten() {
        collect_commands(value, &mut items);
    }
    if items.is_empty() {
        return;
    }

    let mut chars = Vec::new();
    let mut map: Vec<Option<Pos>> = Vec::new();
    for (value, style, mark) in items {
        if !chars.is_empty() {
            let after_previous = map.last().copied().flatten().map(Pos::next);
            chars.push('\n');
            map.push(after_previous);
        }
        chars.extend(value.chars());
        map.extend(
            scalar_positions(source, value, style, mark)
                .into_iter()
                .map(Some),
        );
    }
    scripts.push(Script::new(name, chars, map));
}

/// Collects the command scalars of a script value. Nested sequences are
/// flattened like GitLab does; `!reference` tags and aliases are skipped.
fn collect_commands<'a>(node: &'a Node, out: &mut Vec<(&'a str, TScalarStyle, Marker)>) {
    match node {
        Node::Scalar {
            value,
            style,
            mark,
            reference: false,
        } => {
            let is_null =
                *style == TScalarStyle::Plain && matches!(value.as_str(), "" | "~" | "null");
            if !is_null {
                out.push((value, *style, *mark));
            }
        }
        Node::Sequence {
            items,
            reference: false,
        } => {
            for item in items {
                collect_commands(item, out);
            }
        }
        _ => {}
    }
}

/// Returns the source position of every char of a decoded scalar.
///
/// The scalar is aligned with its source text char by char, accounting for
/// quotes, escapes, block indentation and line folding. If the alignment
/// breaks down, the remaining chars map to the last matched position.
fn scalar_positions(source: &Source, value: &str, style: TScalarStyle, mark: Marker) -> Vec<Pos> {
    let line = mark.line().saturating_sub(1);
    let col = mark.col();
    let mut cursor = match style {
        TScalarStyle::Plain => Cursor::new(source, line, col, None),
        TScalarStyle::SingleQuoted | TScalarStyle::DoubleQuoted => {
            Cursor::new(source, line, col + 1, None)
        }
        TScalarStyle::Literal | TScalarStyle::Folded => {
            // The marker points at the first content line; leading blank
            // lines are part of the value but come before the marker.
            let leading_breaks = value.chars().take_while(|c| *c == '\n').count();
            let first = line.saturating_sub(leading_breaks);
            let start = col.min(source.line(first).len());
            Cursor::new(source, first, start, Some(col))
        }
    };

    let length = value.chars().count();
    let mut positions = Vec::with_capacity(length);
    'chars: for c in value.chars() {
        loop {
            let Some(s) = cursor.peek() else {
                break 'chars;
            };
            if style == TScalarStyle::DoubleQuoted && s == '\\' {
                match cursor.peek_ahead(1) {
                    // Escaped line break: contributes nothing to the value.
                    None => {
                        cursor.advance();
                        cursor.advance();
                        while matches!(cursor.peek(), Some(' ' | '\t')) {
                            cursor.advance();
                        }
                        continue;
                    }
                    Some(escape) => {
                        positions.push(cursor.pos());
                        let width = match escape {
                            'x' => 4,
                            'u' => 6,
                            'U' => 10,
                            _ => 2,
                        };
                        for _ in 0..width {
                            cursor.advance_in_line();
                        }
                        continue 'chars;
                    }
                }
            }
            if s == c {
                positions.push(cursor.pos());
                cursor.advance();
                if style == TScalarStyle::SingleQuoted && c == '\'' && cursor.peek() == Some('\'') {
                    cursor.advance();
                }
                continue 'chars;
            }
            if s == '\n' && c == ' ' {
                // A folded line break.
                positions.push(cursor.pos());
                cursor.advance();
                continue 'chars;
            }
            if matches!(s, ' ' | '\t' | '\n') {
                // Indentation or whitespace removed by folding.
                cursor.advance();
                continue;
            }
            break 'chars;
        }
    }

    let fallback = positions.last().copied().unwrap_or(Pos { line, col });
    positions.resize(length, fallback);
    positions
}

struct Cursor<'a> {
    source: &'a Source,
    line: usize,
    col: usize,
    /// Content indentation of block scalars, skipped at every line start.
    indent: Option<usize>,
}

impl<'a> Cursor<'a> {
    fn new(source: &'a Source, line: usize, col: usize, indent: Option<usize>) -> Self {
        Cursor {
            source,
            line,
            col,
            indent,
        }
    }

    fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
        }
    }

    /// The char at the cursor, `'\n'` at a line end, or `None` at the end of the file.
    fn peek(&self) -> Option<char> {
        let line = self.source.line(self.line);
        if self.col < line.len() {
            Some(line[self.col])
        } else if self.line + 1 < self.source.lines.len() {
            Some('\n')
        } else {
            None
        }
    }

    fn peek_ahead(&self, n: usize) -> Option<char> {
        self.source.line(self.line).get(self.col + n).copied()
    }

    fn advance_in_line(&mut self) {
        if self.col < self.source.line(self.line).len() {
            self.col += 1;
        }
    }

    fn advance(&mut self) {
        if self.col < self.source.line(self.line).len() {
            self.col += 1;
        } else if self.line + 1 < self.source.lines.len() {
            self.line += 1;
            self.col = self
                .indent
                .map_or(0, |indent| indent.min(self.source.line(self.line).len()));
        }
    }
}

enum Node {
    Scalar {
        value: String,
        style: TScalarStyle,
        mark: Marker,
        reference: bool,
    },
    Sequence {
        items: Vec<Node>,
        reference: bool,
    },
    Map(Vec<(Node, Node)>),
    Alias,
}

impl Node {
    fn as_str(&self) -> Option<&str> {
        match self {
            Node::Scalar { value, .. } => Some(value),
            _ => None,
        }
    }

    fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries) => entries
                .iter()
                .find(|(k, _)| k.as_str() == Some(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }
}

enum Frame {
    Sequence {
        items: Vec<Node>,
        reference: bool,
    },
    Map {
        entries: Vec<(Node, Node)>,
        key: Option<Node>,
    },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    documents: Vec<Node>,
}

impl Builder {
    fn push(&mut self, node: Node) {
        match self.stack.last_mut() {
            Some(Frame::Sequence { items, .. }) => items.push(node),
            Some(Frame::Map { entries, key }) => match key.take() {
                Some(key) => entries.push((key, node)),
                None => *key = Some(node),
            },
            None => self.documents.push(node),
        }
    }

    fn close(&mut self) {
        let node = match self.stack.pop() {
            Some(Frame::Sequence { items, reference }) => Node::Sequence { items, reference },
            Some(Frame::Map { entries, .. }) => Node::Map(entries),
            None => return,
        };
        self.push(node);
    }
}

fn is_reference(tag: &Option<Tag>) -> bool {
    tag.as_ref()
        .is_some_and(|tag| tag.handle == "!" && tag.suffix == "reference")
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, event: Event, mark: Marker) {
        match event {
            Event::Scalar(value, style, _, tag) => self.push(Node::Scalar {
                value,
                style,
                mark,
                reference: is_reference(&tag),
            }),
            Event::SequenceStart(_, tag) => self.stack.push(Frame::Sequence {
                items: Vec::new(),
                reference: is_reference(&tag),
            }),
            Event::MappingStart(..) => self.stack.push(Frame::Map {
                entries: Vec::new(),
                key: None,
            }),
            Event::SequenceEnd | Event::MappingEnd => self.close(),
            Event::Alias(_) => self.push(Node::Alias),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts(text: &str) -> (Source, Vec<Script>) {
        parse(text)
    }

    /// Returns the YAML position of the first occurrence of `needle` in the script.
    fn source_of(script: &Script, needle: &str) -> Pos {
        let byte = script.text.find(needle).expect("needle in script");
        let offset = script.text[..byte].chars().count();
        script.to_source(offset).expect("mapped")
    }

    #[test]
    fn combines_before_script_and_script() {
        let (_, scripts) = scripts(
            "job:\n  script:\n    - echo main\n  before_script: echo setup\n  after_script:\n    - echo done\n",
        );
        let names: Vec<_> = scripts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["job/script", "job/after_script"]);
        assert_eq!(scripts[0].text, "echo setup\necho main");
        assert_eq!(scripts[1].text, "echo done");
    }

    #[test]
    fn maps_plain_and_literal_items() {
        let text = "deploy:\n  script:\n    - echo \"Deploying ${PACKAGE_DIR}\"\n    - |\n      echo \"jojo\"\n      echo $HI\n";
        let (_, scripts) = scripts(text);
        let script = &scripts[0];
        assert_eq!(
            script.text,
            "echo \"Deploying ${PACKAGE_DIR}\"\necho \"jojo\"\necho $HI\n"
        );
        assert_eq!(
            source_of(script, "${PACKAGE_DIR}"),
            Pos { line: 2, col: 22 }
        );
        assert_eq!(source_of(script, "\"jojo\""), Pos { line: 4, col: 11 });
        assert_eq!(source_of(script, "$HI"), Pos { line: 5, col: 11 });
    }

    #[test]
    fn maps_quoted_scalars() {
        let text = "job:\n  script:\n    - 'it''s $A'\n    - \"a\\tb $B \\\"q\\\"\"\n";
        let (_, scripts) = scripts(text);
        let script = &scripts[0];
        assert_eq!(script.text, "it's $A\na\tb $B \"q\"");
        assert_eq!(source_of(script, "$A"), Pos { line: 2, col: 13 });
        assert_eq!(source_of(script, "\tb"), Pos { line: 3, col: 8 });
        assert_eq!(source_of(script, "$B"), Pos { line: 3, col: 12 });
        assert_eq!(source_of(script, "\"q"), Pos { line: 3, col: 15 });
    }

    #[test]
    fn maps_folded_and_leading_blank_lines() {
        let text = "job:\n  script:\n    - >\n      echo a\n      $B\n\n      echo $C\n    - |\n\n      echo $D\n";
        let (_, scripts) = scripts(text);
        let script = &scripts[0];
        assert_eq!(script.text, "echo a $B\necho $C\n\n\necho $D\n");
        assert_eq!(source_of(script, "$B"), Pos { line: 4, col: 6 });
        assert_eq!(source_of(script, "$C"), Pos { line: 6, col: 11 });
        assert_eq!(source_of(script, "$D"), Pos { line: 9, col: 11 });
    }

    #[test]
    fn maps_multibyte_columns_to_utf16() {
        let text = "job:\n  script: echo \"ä😀\" $X\n";
        let (source, scripts) = scripts(text);
        let pos = source_of(&scripts[0], "$X");
        assert_eq!(pos, Pos { line: 1, col: 20 });
        assert_eq!(source.utf16_col(pos), 21);
        assert_eq!(source.pos(1, 21), pos);
    }

    #[test]
    fn skips_references_reserved_keys_and_other_properties() {
        let text = "variables:\n  script: echo no\nworkflow:\n  script: echo no\njob:\n  image: alpine\n  script:\n    - !reference [.setup, script]\n    - echo yes\nother:\n  script: !reference [.setup, script]\n";
        let (_, scripts) = scripts(text);
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, "job/script");
        assert_eq!(scripts[0].text, "echo yes");
    }

    #[test]
    fn handles_globals_hooks_nested_lists_and_spec_documents() {
        let text = "spec:\n  inputs: {}\n---\nbefore_script: echo global\njob:\n  hooks:\n    pre_get_sources_script: echo hook\n  script:\n    - [echo one, [echo two]]\n";
        let (_, scripts) = scripts(text);
        let found: Vec<_> = scripts
            .iter()
            .map(|s| (s.name.as_str(), s.text.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("before_script", "echo global"),
                ("job/script", "echo one\necho two"),
                ("job/hooks/pre_get_sources_script", "echo hook"),
            ]
        );
    }

    #[test]
    fn keeps_scripts_before_a_syntax_error() {
        let text = "job:\n  script:\n    - echo ok\nbroken: [\n";
        let (_, scripts) = scripts(text);
        assert_eq!(scripts[0].text, "echo ok");
    }

    #[test]
    fn translates_positions_both_ways() {
        let text = "job:\n  script:\n    - echo $A\n    - echo $B\n";
        let (source, scripts) = scripts(text);
        let script = scripts[0].clone_with_directive();
        // `$B` is on line 1 of the original script, line 2 after the directive.
        let offset = script.offset(2, 5);
        assert_eq!(
            script.source_range(offset, offset + 2),
            Some((Pos { line: 3, col: 11 }, Pos { line: 3, col: 13 }))
        );
        let back = script.offset_of_source(source.pos(3, 12)).unwrap();
        assert_eq!(script.position(back), (2, 6));
        // End of line: just past the last char still maps into the script.
        let end = script.offset_of_source(Pos { line: 3, col: 13 }).unwrap();
        assert_eq!(script.position(end), (2, 7));
        // The synthetic directive line has no source position.
        assert_eq!(script.source_range(0, 3), None);
        assert_eq!(script.offset_of_source(Pos { line: 0, col: 0 }), None);
    }

    #[test]
    fn reserved_keys_match_injection_query() {
        let query = include_str!("../../languages/gitlab-ci/injections.scm");
        let prefix = "(#not-match? @_job \"^[\\\"']?(";
        let lists: Vec<Vec<&str>> = query
            .lines()
            .filter_map(|line| line.trim().strip_prefix(prefix))
            .map(|rest| rest.split(')').next().unwrap().split('|').collect())
            .collect();
        assert!(!lists.is_empty());
        for list in lists {
            assert_eq!(list, RESERVED_KEYS);
        }
    }

    impl Script {
        fn clone_with_directive(&self) -> Script {
            Script::new(self.name.clone(), self.chars.clone(), self.map.clone())
                .prepend_line("# shellcheck shell=sh")
        }
    }
}
