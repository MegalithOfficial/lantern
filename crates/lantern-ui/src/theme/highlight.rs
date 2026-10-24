//! What the Java grammar of GtkSourceView leaves plain: class names, method names and
//! constants. A small lexer finds them and tags are laid over the syntax colours. It only
//! needs to know what is code and what is a string or a comment, not what the code means.

use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Type,
    Call,
    Constant,
}

const KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "var",
    "record",
    "sealed",
    "permits",
    "yield",
    "true",
    "false",
    "null",
];

/// Spans in characters, the unit text buffers count in.
pub fn spans(code: &str) -> Vec<(i32, i32, Kind)> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut previous = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '"' | '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != c && chars[i] != '\n' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            }
            '@' => {
                // An annotation, the grammar has its own colour for those.
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                    i += 1;
                }
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if KEYWORDS.contains(&word.as_str()) {
                    previous = word;
                    continue;
                }
                let next = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
                let upper = word.starts_with(|c: char| c.is_uppercase());
                let shouting = word.len() > 1 && word.chars().any(|c| c.is_uppercase()) && !word.chars().any(|c| c.is_lowercase());
                let kind = match next {
                    Some('(') if previous == "new" || upper => Some(Kind::Type),
                    Some('(') => Some(Kind::Call),
                    _ if shouting => Some(Kind::Constant),
                    _ if upper => Some(Kind::Type),
                    _ => None,
                };
                if let Some(kind) = kind {
                    out.push((start as i32, i as i32, kind));
                }
                previous = word;
            }
            c if c.is_ascii_digit() => {
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Lays the colours over whatever is in the buffer.
pub fn apply(buffer: &gtk::TextBuffer) {
    let table = buffer.tag_table();
    for (name, color) in [("lt-type", "#ffa657"), ("lt-call", "#be8fff"), ("lt-constant", "#79c0ff")] {
        if table.lookup(name).is_none() {
            buffer.create_tag(Some(name), &[("foreground", &color)]);
        }
    }
    let (start, end) = buffer.bounds();
    let text = buffer.text(&start, &end, true);
    for (from, to, kind) in spans(&text) {
        let name = match kind {
            Kind::Type => "lt-type",
            Kind::Call => "lt-call",
            Kind::Constant => "lt-constant",
        };
        buffer.apply_tag_by_name(name, &buffer.iter_at_offset(from), &buffer.iter_at_offset(to));
    }
}

/// Marks the lines of the method that is being looked at, and only those.
pub fn focus(buffer: &gtk::TextBuffer, first_line: i32, lines: i32) {
    if buffer.tag_table().lookup("lt-focus").is_none() {
        buffer.create_tag(Some("lt-focus"), &[("paragraph-background", &"#151b23")]);
    }
    let (start, end) = buffer.bounds();
    buffer.remove_tag_by_name("lt-focus", &start, &end);
    buffer.remove_tag_by_name("lt-line", &start, &end);
    let (Some(from), Some(to)) = (buffer.iter_at_line(first_line), buffer.iter_at_line(first_line + lines)) else { return };
    buffer.apply_tag_by_name("lt-focus", &from, &to);
}

/// Marks the one line a link led to, over the method it is in.
pub fn line(buffer: &gtk::TextBuffer, line: i32) {
    let table = buffer.tag_table();
    let tag = table.lookup("lt-line").unwrap_or_else(|| buffer.create_tag(Some("lt-line"), &[("paragraph-background", &"#1d3150")]).expect("new tag"));
    // Above the method mark and the syntax colours.
    tag.set_priority(table.size() - 1);
    let (start, end) = buffer.bounds();
    buffer.remove_tag_by_name("lt-line", &start, &end);
    let (Some(from), Some(to)) = (buffer.iter_at_line(line), buffer.iter_at_line(line + 1)) else { return };
    buffer.apply_tag_by_name("lt-line", &from, &to);
}

#[cfg(test)]
mod tests {
    use super::{Kind, spans};

    fn kinds(code: &str) -> Vec<(String, Kind)> {
        let chars: Vec<char> = code.chars().collect();
        spans(code).into_iter().map(|(a, b, k)| (chars[a as usize..b as usize].iter().collect(), k)).collect()
    }

    #[test]
    fn finds_types_calls_and_constants_but_not_in_strings() {
        let found = kinds("Lamp l = new Lamp(room); // Wick here\nl.setGlow(Glow.BRIGHT); String s = \"Foo bar(\"; @Override int MAX_GLOW = 150;");
        assert_eq!(
            found,
            [
                ("Lamp".to_owned(), Kind::Type),
                ("Lamp".to_owned(), Kind::Type),
                ("setGlow".to_owned(), Kind::Call),
                ("Glow".to_owned(), Kind::Type),
                ("BRIGHT".to_owned(), Kind::Constant),
                ("String".to_owned(), Kind::Type),
                ("MAX_GLOW".to_owned(), Kind::Constant),
            ]
        );
    }
}
