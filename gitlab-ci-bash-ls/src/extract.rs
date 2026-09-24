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
pub(crate) const RESERVED_KEYS: &[&str] = &[
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

/// The full, half-open source span of a decoded character (including escape syntax).
#[derive(Clone, Copy, Debug)]
struct SourceSpan {
    start: Pos,
    end: Pos,
}

impl SourceSpan {
    fn single(start: Pos) -> Self {
        SourceSpan {
            start,
            end: start.next(),
        }
    }
}

/// A virtual shell document assembled from one or more YAML script entries.
#[derive(Debug)]
pub struct Script {
    pub name: String,
    pub text: String,
    chars: Vec<char>,
    /// Source span of every char in `chars`; `None` for synthetic text.
    map: Vec<Option<SourceSpan>>,
    line_starts: Vec<usize>,
}

impl Script {
    fn new(name: String, chars: Vec<char>, map: Vec<Option<SourceSpan>>) -> Self {
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
            Some(span) => span.map(|span| span.start),
            None => self.map.iter().rev().flatten().next().map(|span| span.end),
        }
    }

    /// Maps the virtual char range `start..end` to a YAML range.
    pub fn source_range(&self, start: usize, end: usize) -> Option<(Pos, Pos)> {
        let from = self.to_source(start)?;
        let to = if end > start {
            self.map
                .get(end - 1)
                .copied()
                .flatten()
                .map_or(from, |span| span.end)
        } else {
            from
        };
        Some((from, to.max(from)))
    }

    /// Returns the virtual char offset for a YAML position inside this script.
    /// A position just after the last char of a line also matches, so that
    /// completion at the end of a command works. Positions inside a YAML escape
    /// are not decoded character boundaries and deliberately do not match.
    pub fn offset_of_source(&self, pos: Pos) -> Option<usize> {
        self.map
            .iter()
            .position(|span| span.is_some_and(|span| span.start == pos))
            .or_else(|| {
                self.map
                    .iter()
                    .position(|span| span.is_some_and(|span| span.end == pos))
                    .map(|i| i + 1)
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
    let mut map: Vec<Option<SourceSpan>> = Vec::new();
    for (value, style, mark) in items {
        if !chars.is_empty() {
            // The separator has no source width: do not include a scalar's closing quote.
            let after_previous = map.last().copied().flatten().map(|span| SourceSpan {
                start: span.end,
                end: span.end,
            });
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

/// Returns the source span of every char of a decoded scalar.
///
/// The scalar is aligned with its source text char by char, accounting for
/// quotes, escapes, block indentation and line folding. If the alignment
/// breaks down, the remaining chars map to the last matched span.
fn scalar_positions(
    source: &Source,
    value: &str,
    style: TScalarStyle,
    mark: Marker,
) -> Vec<SourceSpan> {
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
    let mut in_flow_break = false;
    'chars: for c in value.chars() {
        loop {
            let Some(s) = cursor.peek() else {
                break 'chars;
            };
            if cursor.indent.is_none() {
                let source_line = source.line(cursor.line);
                if matches!(s, ' ' | '\t')
                    && source_line[cursor.col..]
                        .iter()
                        .all(|s| matches!(s, ' ' | '\t'))
                {
                    // Flow folding discards trailing whitespace as well as continuation indentation.
                    cursor.col = source_line.len();
                    continue;
                }
                if s == '\n' && c == '\n' && !in_flow_break {
                    // N flow line breaks decode to N-1 newlines (or a space for N=1).
                    cursor.advance();
                    in_flow_break = true;
                    continue;
                }
            }
            if style == TScalarStyle::DoubleQuoted && s == '\\' {
                match cursor.peek_ahead(1) {
                    // Escaped line break: contributes nothing to the value.
                    None => {
                        cursor.advance();
                        cursor.advance();
                        in_flow_break = true;
                        continue;
                    }
                    Some(escape) => {
                        in_flow_break = false;
                        let start = cursor.pos();
                        let width = match escape {
                            'x' => 4,
                            'u' => 6,
                            'U' => 10,
                            _ => 2,
                        };
                        for _ in 0..width {
                            cursor.advance_in_line();
                        }
                        positions.push(SourceSpan {
                            start,
                            end: cursor.pos(),
                        });
                        continue 'chars;
                    }
                }
            }
            if s == c {
                in_flow_break = s == '\n';
                let mut span = SourceSpan::single(cursor.pos());
                cursor.advance();
                if style == TScalarStyle::SingleQuoted && c == '\'' && cursor.peek() == Some('\'') {
                    cursor.advance();
                    span.end = cursor.pos();
                }
                positions.push(span);
                continue 'chars;
            }
            if s == '\n' && c == ' ' {
                // A folded line break.
                in_flow_break = true;
                positions.push(SourceSpan::single(cursor.pos()));
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

    let fallback = positions
        .last()
        .copied()
        .unwrap_or_else(|| SourceSpan::single(Pos { line, col }));
    positions.resize(length, fallback);
    positions
}

struct Cursor<'a> {
    source: &'a Source,
    line: usize,
    col: usize,
    /// Block content indentation; `None` trims all flow-scalar continuation indentation.
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
            let line = self.source.line(self.line);
            self.col = self.indent.map_or_else(
                || line.iter().take_while(|c| matches!(c, ' ' | '\t')).count(),
                |indent| indent.min(line.len()),
            );
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
    fn maps_escaped_space_after_folded_continuation() {
        for (trailing, indent) in [("", "    "), (" \t", "    "), ("", "    \t")] {
            let text = format!("job:\n  script: \"echo{trailing}\n{indent}\\x20$FOO\"\n");
            let (_, scripts) = scripts(&text);
            let script = &scripts[0];
            let col = indent.chars().count();
            assert_eq!(script.text, "echo  $FOO");
            let range = (
                Pos {
                    line: 2,
                    col: col + 4,
                },
                Pos {
                    line: 2,
                    col: col + 8,
                },
            );
            assert_eq!(script.source_range(6, 10), Some(range));
            assert_eq!(script.offset_of_source(range.0), Some(6));
            assert_eq!(script.offset_of_source(range.1), Some(10));
            assert_eq!(
                script.source_range(5, 6),
                Some((Pos { line: 2, col }, range.0))
            );
            assert_eq!(script.offset_of_source(Pos { line: 2, col: 0 }), None);
            assert_replacement(&text, script, 6, 10, "$BAR", "echo  $BAR");
        }
    }

    #[test]
    fn maps_complete_escape_spans_and_rejects_interior_positions() {
        for (encoded, decoded) in [
            ("\\x4f", 'O'),
            ("\\u004f", 'O'),
            ("\\U0000004f", 'O'),
            ("\\U0001f600", '😀'),
            ("\\t", '\t'),
            ("\\\"", '"'),
            ("\\\\", '\\'),
        ] {
            let text = format!("job:\n  script: \"echo $FO{encoded}\"\n");
            let (source, scripts) = scripts(&text);
            let script = &scripts[0];
            let start = Pos { line: 1, col: 19 };
            let end = Pos {
                line: 1,
                col: 19 + encoded.chars().count(),
            };
            assert_eq!(script.text, format!("echo $FO{decoded}"));
            assert_eq!(script.source_range(8, 9), Some((start, end)));
            assert_eq!(
                script.source_range(5, 9),
                Some((Pos { line: 1, col: 16 }, end))
            );
            assert_eq!(script.source_range(9, 9), Some((end, end)));
            assert_eq!(script.offset_of_source(start), Some(8));
            assert_eq!(script.offset_of_source(end), Some(9));
            for col in start.col + 1..end.col {
                assert_eq!(script.offset_of_source(Pos { line: 1, col }), None);
            }
            // Neither the opening quote nor a position after the closing quote is script text.
            assert_eq!(script.offset_of_source(Pos { line: 1, col: 10 }), None);
            assert_eq!(script.offset_of_source(end.next()), None);
            assert_eq!(script.position(9), (0, 8 + decoded.len_utf16()));
            assert_eq!(script.offset(0, 8 + decoded.len_utf16()), 9);
            assert_eq!(source.pos(1, source.utf16_col(end)), end);
            assert_replacement(&text, script, 5, 9, "$BAR", "echo $BAR");
        }
    }

    #[test]
    fn maps_doubled_single_quote_end() {
        let text = "job:\n  script: 'echo it'''\n";
        let (_, scripts) = scripts(text);
        let script = &scripts[0];
        let start = Pos { line: 1, col: 18 };
        let end = Pos { line: 1, col: 20 };
        assert_eq!(script.text, "echo it'");
        assert_eq!(script.source_range(7, 8), Some((start, end)));
        assert_eq!(script.source_range(8, 8), Some((end, end)));
        assert_eq!(script.offset_of_source(start), Some(7));
        assert_eq!(script.offset_of_source(start.next()), None);
        assert_eq!(script.offset_of_source(end), Some(8));
        assert_eq!(script.offset_of_source(end.next()), None);
        assert_replacement(text, script, 5, 8, "$BAR", "echo $BAR");
    }

    /// Exercise a completion-style edit and ensure no escape suffix or YAML quote is left behind.
    fn assert_replacement(
        text: &str,
        script: &Script,
        start: usize,
        end: usize,
        replacement: &str,
        expected: &str,
    ) {
        let (from, to) = script.source_range(start, end).expect("mapped replacement");
        let byte_offset = |pos: Pos| {
            let line_start: usize = text
                .split_inclusive('\n')
                .take(pos.line)
                .map(str::len)
                .sum();
            line_start
                + text[line_start..]
                    .chars()
                    .take(pos.col)
                    .map(char::len_utf8)
                    .sum::<usize>()
        };
        let mut edited = text.to_owned();
        edited.replace_range(byte_offset(from)..byte_offset(to), replacement);
        let (_, scripts) = scripts(&edited);
        assert_eq!(scripts.len(), 1, "{edited}");
        assert_eq!(scripts[0].text, expected, "{edited}");
    }

    #[test]
    fn maps_escaped_continuations_and_preserves_block_indentation() {
        for (text, decoded, line, col) in [
            (
                "job:\n  script: \"echo\\\n    \\x20$FOO\"\n",
                "echo $FOO",
                2,
                8,
            ),
            (
                "job:\n  script: \"echo\n\n    \\x20$FOO\"\n",
                "echo\n $FOO",
                3,
                8,
            ),
            ("job:\n  script: echo\n    $FOO\n", "echo $FOO", 2, 4),
            ("job:\n  script: 'echo\n    $FOO'\n", "echo $FOO", 2, 4),
            (
                "job:\n  script: |-\n    echo\n      $FOO\n",
                "echo\n  $FOO",
                3,
                6,
            ),
            (
                "job:\n  script: >-\n    echo\n      $FOO\n",
                "echo\n  $FOO",
                3,
                6,
            ),
        ] {
            let (_, scripts) = scripts(text);
            let script = &scripts[0];
            assert_eq!(script.text, decoded);
            let start = decoded.find("$FOO").unwrap();
            let from = Pos { line, col };
            let to = Pos { line, col: col + 4 };
            assert_eq!(script.source_range(start, start + 4), Some((from, to)));
            assert_eq!(script.offset_of_source(from), Some(start));
            assert_eq!(script.offset_of_source(to), Some(start + 4));
            assert_replacement(
                text,
                script,
                start,
                start + 4,
                "$BAR",
                &decoded.replace("$FOO", "$BAR"),
            );
        }
    }

    #[test]
    fn maps_escape_endpoints_through_script_composition() {
        // YAML order differs from execution order, so the source map is not sorted.
        let text = "job:\n  script: \"echo $BA\\x52\"\n  before_script: \"echo $FO\\u004f\"\n";
        let (_, scripts) = scripts(text);
        let script = &scripts[0];
        assert_eq!(script.text, "echo $FOO\necho $BAR");
        let before_end = Pos { line: 2, col: 32 };
        assert_eq!(
            script.source_range(5, 9),
            Some((Pos { line: 2, col: 23 }, before_end))
        );
        assert_eq!(script.source_range(9, 9), Some((before_end, before_end)));
        // The inserted newline must not consume the closing YAML quote.
        assert_eq!(script.source_range(9, 10), Some((before_end, before_end)));
        assert_eq!(script.offset_of_source(before_end), Some(9));
        assert_eq!(script.offset_of_source(before_end.next()), None);
        assert_eq!(script.offset_of_source(Pos { line: 2, col: 27 }), None);
        let main_end = Pos { line: 1, col: 23 };
        assert_eq!(
            script.source_range(15, 19),
            Some((Pos { line: 1, col: 16 }, main_end))
        );
        assert_eq!(script.offset_of_source(main_end), Some(19));
        assert_replacement(text, script, 5, 9, "$SETUP", "echo $SETUP\necho $BAR");
        assert_replacement(text, script, 15, 19, "$MAIN", "echo $FOO\necho $MAIN");

        let script = script.clone_with_directive();
        let start = script.offset(1, 5);
        let end = script.offset(1, 9);
        assert_eq!(script.offset_of_source(before_end), Some(end));
        assert_eq!(script.source_range(start, end).unwrap().1, before_end);
        assert_eq!(script.source_range(0, 1), None);
    }

    #[test]
    fn maps_escaped_and_literal_astral_characters_with_utf16() {
        let text = "job:\n  script: \"echo 😀\\U0001f600 $FO\\u004f\"\n";
        let (source, scripts) = scripts(text);
        let script = &scripts[0];
        assert_eq!(script.text, "echo 😀😀 $FOO");
        let escape_start = Pos { line: 1, col: 17 };
        let escape_end = Pos { line: 1, col: 27 };
        assert_eq!(script.source_range(6, 7), Some((escape_start, escape_end)));
        assert_eq!(source.utf16_col(escape_start), 18);
        assert_eq!(source.utf16_col(escape_end), 28);
        assert_eq!(script.position(6), (0, 7));
        assert_eq!(script.position(7), (0, 9));
        assert_eq!(script.offset(0, 9), 7);
        assert_eq!(script.offset_of_source(source.pos(1, 28)), Some(7));
        for col in 19..28 {
            assert_eq!(script.offset_of_source(source.pos(1, col)), None);
        }
        let range = (Pos { line: 1, col: 28 }, Pos { line: 1, col: 37 });
        assert_eq!(script.source_range(8, 12), Some(range));
        assert_eq!(source.utf16_col(range.0), 29);
        assert_eq!(source.utf16_col(range.1), 38);
        let end = script.offset_of_source(source.pos(1, 38)).unwrap();
        assert_eq!(end, 12);
        assert_eq!(script.position(end), (0, 14));
        assert_replacement(text, script, 8, 12, "$BAR", "echo 😀😀 $BAR");
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
