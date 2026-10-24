//! What a lookup found, shared by the CLI, the MCP server and the UI. Built as a `Doc` whose
//! parts know what they point at: the CLI and MCP print it as plain text, the UI draws it with
//! links.

use anyhow::{Result, bail};
use rusqlite::Connection;

use crate::query::{self, CallSite, MethodRef, Needle, Site};

pub enum Query {
    Class { name: String },
    Callers { target: String },
    Callees { target: String },
    Refs { target: String },
    Key { ident: String },
    Strings { pattern: String },
    Search { query: String, limit: usize },
    Source { class: String, method: Option<String> },
    Topic { seed: String, depth: usize },
}

pub fn run(conn: &Connection, query: &Query) -> Result<String> {
    Ok(document(conn, query)?.to_text())
}

pub fn document(conn: &Connection, query: &Query) -> Result<Doc> {
    match query {
        Query::Class { name } => class(conn, name),
        Query::Callers { target } => callers(conn, target),
        Query::Callees { target } => callees(conn, target),
        Query::Refs { target } => refs(conn, target),
        Query::Key { ident } => key(conn, ident),
        Query::Strings { pattern } => strings(conn, pattern),
        Query::Search { query, limit } => search(conn, query, *limit),
        Query::Source { class, method } => source(conn, class, method.as_deref()),
        Query::Topic { seed, depth } => topic(conn, seed, *depth),
    }
}

/// A piece of a line.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text(String),
    /// Said in passing: descriptors, line numbers, counts, labels.
    Muted(String),
    /// `reads` or `writes`.
    Verb(String),
    /// A class, or a member as `Class.member`, by its full name. With a site the link goes to
    /// the line where the method does what the lookup was about.
    Symbol {
        text: String,
        target: String,
        site: Option<Site>,
    },
    /// `:230`, the line of a site in the method that `target` names.
    At {
        text: String,
        target: String,
        site: Site,
    },
    /// A registry key or a tag, `ident` without the `#` of a tag reference.
    Key {
        text: String,
        ident: String,
    },
    /// A string constant, `text` quoted the way Rust debug prints it.
    Str {
        text: String,
        value: String,
    },
    /// A file, and the class it holds when it is decompiled source.
    File {
        text: String,
        path: String,
        class: Option<String>,
        line: Option<i64>,
    },
    /// What a text search matched.
    Mark(String),
}

impl Span {
    /// What it reads as. A mark is shown bare, the plain text puts `>>` `<<` around it.
    pub fn text(&self) -> &str {
        match self {
            Span::Text(t) | Span::Muted(t) | Span::Verb(t) | Span::Mark(t) => t,
            Span::Symbol { text, .. } | Span::At { text, .. } | Span::Key { text, .. } | Span::Str { text, .. } | Span::File { text, .. } => text,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Title(String),
    Section(String),
    Line(Vec<Span>),
    Blank,
    /// Decompiled Java, as it is in the file.
    Code(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

impl Doc {
    fn line(&mut self, spans: impl IntoIterator<Item = Span>) {
        self.blocks.push(Block::Line(spans.into_iter().collect()));
    }

    fn blank(&mut self) {
        self.blocks.push(Block::Blank);
    }

    /// The text the CLI prints and the MCP server returns.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::Title(title) => out += &format!("# {title}\n"),
                Block::Section(title) => out += &format!("## {title}\n"),
                Block::Line(spans) => {
                    for span in spans {
                        match span {
                            Span::Mark(word) => out += &format!(">>{word}<<"),
                            span => out += span.text(),
                        }
                    }
                    out.push('\n');
                }
                Block::Blank => out.push('\n'),
                // A method body has no newline at its end, the block after it brings one.
                Block::Code(code) => out += code,
            }
        }
        out
    }
}

pub fn short(class: &str) -> &str {
    class.strip_prefix("net/minecraft/").unwrap_or(class)
}

/// `(Lnet/minecraft/core/BlockPos;I)Z` as `(BlockPos, int) boolean`.
pub fn signature(descriptor: &str) -> String {
    fn one(chars: &mut std::str::Chars) -> Option<String> {
        Some(match chars.next()? {
            'Z' => "boolean".into(),
            'B' => "byte".into(),
            'C' => "char".into(),
            'S' => "short".into(),
            'I' => "int".into(),
            'J' => "long".into(),
            'F' => "float".into(),
            'D' => "double".into(),
            'V' => "void".into(),
            '[' => format!("{}[]", one(chars)?),
            'L' => {
                let name: String = chars.take_while(|c| *c != ';').collect();
                name.rsplit(['/', '$']).next().unwrap_or(&name).to_owned()
            }
            _ => return None,
        })
    }

    let Some((params, ret)) = descriptor.strip_prefix('(').and_then(|d| d.split_once(')')) else {
        return descriptor.to_owned();
    };
    let mut chars = params.chars();
    let mut out = Vec::new();
    while let Some(param) = one(&mut chars) {
        out.push(param);
    }
    format!("({}) {}", out.join(", "), one(&mut ret.chars()).unwrap_or_default())
}

fn at(line: Option<i64>) -> String {
    line.map(|l| format!(":{l}")).unwrap_or_default()
}

fn method_label(method: &MethodRef) -> String {
    format!("{}.{}", short(&method.class), method.name)
}

/// A method as a link, named the short way and pointing at the full name.
fn method_span(method: &MethodRef) -> Span {
    Span::Symbol { text: method_label(method), target: target(method), site: None }
}

fn target(method: &MethodRef) -> String {
    format!("{}.{}", method.class, method.name)
}

/// A method that does `needle` at `line`, and the line itself, both leading there.
fn site_spans(method: &MethodRef, line: Option<i64>, needle: Needle) -> [Span; 2] {
    let site = line.map(|line| Site { method_id: method.id, line, needle });
    let at = match &site {
        Some(site) => Span::At { text: at(line), target: target(method), site: site.clone() },
        None => muted(""),
    };
    [Span::Symbol { text: method_label(method), target: target(method), site }, at]
}

fn class_span(class: &str) -> Span {
    symbol(short(class), class)
}

fn symbol(text: impl Into<String>, target: impl Into<String>) -> Span {
    Span::Symbol { text: text.into(), target: target.into(), site: None }
}

fn text(value: impl Into<String>) -> Span {
    Span::Text(value.into())
}

fn muted(value: impl Into<String>) -> Span {
    Span::Muted(value.into())
}

/// The file of a class, opened in Browse at the line.
fn file(location: &query::Location, class: &str) -> Span {
    Span::File { text: location.to_string(), path: location.path.clone(), class: Some(class.to_owned()), line: location.line }
}

/// `items` with `, ` between them.
fn listed(items: impl IntoIterator<Item = Span>) -> Vec<Span> {
    let mut out = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            out.push(text(", "));
        }
        out.push(item);
    }
    out
}

/// A tag member or a key: `#minecraft:dirt` looks up `minecraft:dirt`.
fn key_span(ident: &str) -> Span {
    Span::Key { text: ident.to_owned(), ident: ident.trim_start_matches('#').to_owned() }
}

/// A full text search snippet, `>>word<<` marks a match. Shift operators look the same, a
/// mark is only taken when it closes before the next space.
fn snippet(value: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut rest = value;
    let mut plain = String::new();
    while let Some(open) = rest.find(">>") {
        let after = &rest[open + 2..];
        match after.find("<<").filter(|close| !after[..*close].contains(char::is_whitespace) && *close > 0) {
            Some(close) => {
                plain.push_str(&rest[..open]);
                if !plain.is_empty() {
                    out.push(text(std::mem::take(&mut plain)));
                }
                out.push(Span::Mark(after[..close].to_owned()));
                rest = &after[close + 2..];
            }
            None => {
                plain.push_str(&rest[..open + 2]);
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        out.push(text(plain));
    }
    out
}

fn class(conn: &Connection, name: &str) -> Result<Doc> {
    let info = query::class_info(conn, name)?;
    let mut doc = Doc::default();
    doc.line([symbol(&info.name, &info.name)]);
    if let Some(location) = query::class_location(conn, &info.name) {
        doc.line([muted("  file: "), file(&location, &info.name)]);
    }
    if let Some(parent) = &info.super_class {
        doc.line([muted("  extends "), symbol(parent, parent)]);
    }
    for interface in &info.interfaces {
        doc.line([muted("  implements "), symbol(interface, interface)]);
    }
    if !info.subclasses.is_empty() {
        doc.line([muted("  subtypes: ")].into_iter().chain(listed(info.subclasses.iter().map(|s| class_span(s)))));
    }
    if !info.inner.is_empty() {
        let inner = info.inner.iter().map(|s| symbol(s.rsplit('$').next().unwrap_or(s), s));
        doc.line([muted("  inner: ")].into_iter().chain(listed(inner)));
    }
    doc.line([muted("  fields:")]);
    for field in &info.fields {
        let mut line = vec![text("    "), symbol(&field.name, format!("{}.{}", info.name, field.name))];
        line.push(muted(format!(": {}", field.descriptor)));
        if let Some(value) = &field.constant {
            line.extend([text(" = "), text(value.clone())]);
        }
        if let Some(key) = &field.key {
            line.extend([muted("  -> "), key_span(key)]);
        }
        doc.line(line);
    }
    doc.line([muted("  methods:")]);
    for method in &info.methods {
        let mut line = vec![text("    "), symbol(&method.name, format!("{}.{}", info.name, method.name)), muted(method.descriptor.clone())];
        if let Some(parent) = &method.overrides {
            line.extend([muted("  overrides "), class_span(parent)]);
        }
        doc.line(line);
    }

    let others = query::find_classes(conn, name)?.into_iter().filter(|c| c.id != info.id).take(8).collect::<Vec<_>>();
    if !others.is_empty() {
        doc.blank();
        doc.line([muted("also matching: ")].into_iter().chain(listed(others.iter().map(|c| class_span(&c.name)))));
    }
    Ok(doc)
}

fn write_sites(doc: &mut Doc, sites: &[CallSite], callers: bool) {
    let mut hidden = 0;
    for site in sites {
        let shown = if callers { &site.caller } else { &site.callee };
        if !callers && !site.callee.class.starts_with("net/minecraft/") && !site.callee.class.starts_with("com/mojang/") {
            hidden += 1;
            continue;
        }
        let needle = Needle::Call { callee_id: site.callee.id, class: site.callee.class.clone(), name: site.callee.name.clone() };
        let mut line = vec![text("  ")];
        if callers {
            // The caller and its line lead to the call.
            line.extend(site_spans(&site.caller, site.line, needle));
        } else {
            // The callee leads to its own code, the line to the call in the method looked at.
            let [_, at] = site_spans(&site.caller, site.line, needle);
            line.extend([method_span(shown), at]);
        }
        if site.receiver != site.callee.class {
            line.extend([muted(" (on "), class_span(&site.receiver), muted(")")]);
        }
        if site.kind == "lambda" {
            line.push(muted(" [method ref]"));
        }
        doc.line(line);
    }
    if hidden > 0 {
        doc.line([muted(format!("  (+{hidden} calls into java and libraries)"))]);
    }
}

fn callers(conn: &Connection, target: &str) -> Result<Doc> {
    let (class, member) = query::split_member(target)?;
    let mut doc = Doc::default();
    for method in query::find_methods(conn, class, member)? {
        let sites = query::callers(conn, method.id)?;
        doc.line([text("callers of "), method_span(&method), muted(format!("{} ({})", method.descriptor, sites.len()))]);
        write_sites(&mut doc, &sites, true);
        let overriders = query::overriders(conn, method.id)?;
        if !overriders.is_empty() {
            doc.line([muted("  overridden in: ")].into_iter().chain(listed(overriders.iter().map(|m| class_span(&m.class)))));
        }
        doc.blank();
    }
    Ok(doc)
}

fn callees(conn: &Connection, target: &str) -> Result<Doc> {
    let (class, member) = query::split_member(target)?;
    let mut doc = Doc::default();
    for method in query::find_methods(conn, class, member)? {
        let sites = query::callees(conn, method.id)?;
        doc.line([method_span(&method), muted(method.descriptor.clone()), text(" calls")]);
        write_sites(&mut doc, &sites, false);
        doc.blank();
    }
    Ok(doc)
}

fn refs(conn: &Connection, target: &str) -> Result<Doc> {
    let (class, field) = query::split_member(target)?;
    let (field_id, owner) = query::find_field(conn, class, field)?;
    let mut doc = Doc::default();
    doc.line([text("uses of "), symbol(format!("{}.{field}", short(&owner)), format!("{owner}.{field}"))]);
    for usage in query::field_uses(conn, field_id)? {
        let verb = if usage.is_write { " writes" } else { " reads" };
        let needle = Needle::Field { field_id, name: field.to_owned() };
        doc.line([text("  ")].into_iter().chain(site_spans(&usage.method, usage.line, needle)).chain([Span::Verb(verb.to_owned())]));
    }
    Ok(doc)
}

fn key(conn: &Connection, ident: &str) -> Result<Doc> {
    let info = query::key_info(conn, ident)?;
    let mut doc = Doc::default();
    for data in &info.files {
        let path = match query::data_location(conn, &data.path) {
            Some(location) => Span::File { text: location.to_string(), path: location.path, class: None, line: None },
            None => Span::File { text: data.path.clone(), path: data.path.clone(), class: None, line: None },
        };
        doc.line([text("data: "), muted(format!("{} ", data.registry)), key_span(&data.ident)]);
        doc.line([muted("  file: "), path]);
        if data.registry.starts_with("tags/") {
            let members = listed(data.members.iter().map(|m| key_span(m)));
            doc.line([muted(format!("  {} members: ", data.members.len()))].into_iter().chain(members));
        }
    }
    if !info.member_of.is_empty() {
        let tags = info.member_of.iter().map(|(registry, tag)| vec![muted(format!("{registry} ")), key_span(tag)]);
        let mut line = vec![text("listed in: ")];
        for (index, tag) in tags.enumerate() {
            if index > 0 {
                line.push(text(", "));
            }
            line.extend(tag);
        }
        doc.line(line);
    }
    for holder in &info.holders {
        let mut line = vec![text("java: "), symbol(format!("{}.{}", short(&holder.class), holder.field), format!("{}.{}", holder.class, holder.field))];
        if let Some(via) = &holder.via {
            line.extend([muted(" (copied from "), class_span(via), muted(")")]);
        }
        doc.line(line);
        let field_id = query::find_field(conn, &holder.class, &holder.field).ok().map(|(id, _)| id);
        for usage in &holder.uses {
            let verb = if usage.is_write { " writes" } else { " reads" };
            let line = match field_id {
                Some(field_id) => site_spans(&usage.method, usage.line, Needle::Field { field_id, name: holder.field.clone() }),
                None => [method_span(&usage.method), muted(at(usage.line))],
            };
            doc.line([text("  ")].into_iter().chain(line).chain([Span::Verb(verb.to_owned())]));
        }
    }
    Ok(doc)
}

fn strings(conn: &Connection, pattern: &str) -> Result<Doc> {
    let hits = query::strings(conn, pattern, 200)?;
    if hits.is_empty() {
        bail!("no string constant contains `{pattern}`");
    }
    let mut doc = Doc::default();
    for hit in hits {
        let [method, line] = site_spans(&hit.method, hit.line, Needle::Str(hit.value.clone()));
        doc.line([Span::Str { text: format!("{:?}", hit.value), value: hit.value.clone() }, text("  "), method, line]);
    }
    Ok(doc)
}

fn search(conn: &Connection, value: &str, limit: usize) -> Result<Doc> {
    let hits = query::search(conn, value, limit)?;
    if hits.is_empty() {
        bail!("no source matches `{value}`");
    }
    let mut doc = Doc::default();
    for hit in hits {
        let mut line = match &hit.method {
            Some(method) => {
                let mut line = vec![method_span(method)];
                if let Some(location) = query::method_location(conn, method.id) {
                    line.extend([text("  "), file(&location, &method.class)]);
                }
                line
            }
            None => {
                let mut line = vec![class_span(&hit.class), muted(" (class)")];
                if let Some(location) = query::class_location(conn, &hit.class) {
                    line.extend([text("  "), file(&location, &hit.class)]);
                }
                line
            }
        };
        doc.line(std::mem::take(&mut line));
        doc.line([text("    ")].into_iter().chain(snippet(&hit.snippet)));
    }
    Ok(doc)
}

fn source(conn: &Connection, class: &str, method: Option<&str>) -> Result<Doc> {
    let mut doc = Doc::default();
    let Some(member) = method else {
        let code = query::class_source(conn, class)?;
        let hit = query::find_classes(conn, class)?.into_iter().next();
        if let Some((hit, location)) = hit.and_then(|h| query::class_location(conn, &h.name).map(|l| (h, l))) {
            doc.line([muted("// "), file(&location, &hit.name)]);
        }
        doc.blocks.push(Block::Code(code));
        return Ok(doc);
    };
    for method in query::find_methods(conn, class, member)? {
        let Some(code) = query::method_source(conn, method.id)? else { continue };
        if !doc.blocks.is_empty() {
            // Blocks are a blank line apart.
            doc.blank();
            doc.blank();
        }
        doc.line([muted("// "), method_span(&method), muted(method.descriptor.clone())]);
        if let Some(location) = query::method_location(conn, method.id) {
            doc.line([muted("// "), file(&location, &method.class)]);
        }
        doc.blocks.push(Block::Code(code));
    }
    if doc.blocks.is_empty() {
        bail!("`{member}` has no decompiled body (abstract, synthetic, or the index was built without --src)");
    }
    Ok(doc)
}

fn topic(conn: &Connection, seed: &str, depth: usize) -> Result<Doc> {
    let topic = query::topic(conn, seed, depth)?;
    let mut doc = Doc::default();
    doc.blocks.push(Block::Title(topic.seed.clone()));
    doc.blank();
    doc.blocks.push(Block::Section(format!("classes ({})", topic.classes.len())));
    for class in &topic.classes {
        doc.line([text("  "), class_span(class)]);
    }
    for (title, list) in [("used by", &topic.used_by), ("uses", &topic.uses)] {
        if !list.is_empty() {
            doc.blank();
            doc.blocks.push(Block::Section(title.to_owned()));
            for (class, count) in list {
                doc.line([text("  "), muted(format!("{count:>4}")), text("  "), class_span(class)]);
            }
        }
    }
    let member = |class: &str, field: &str| symbol(format!("{class}.{field}"), format!("{class}.{field}"));
    if !topic.keys.is_empty() {
        doc.blank();
        doc.blocks.push(Block::Section("tags and registry keys touched".to_owned()));
        for (class, field, ident) in &topic.keys {
            doc.line([text("  "), member(class, field), text(" = "), key_span(ident)]);
        }
    }
    if !topic.data.is_empty() {
        doc.blank();
        doc.blocks.push(Block::Section("data files".to_owned()));
        for (registry, ident) in &topic.data {
            doc.line([text("  "), muted(format!("{registry} ")), key_span(ident)]);
        }
    }
    if !topic.constants.is_empty() {
        doc.blank();
        doc.blocks.push(Block::Section("constants".to_owned()));
        for (class, field, value) in &topic.constants {
            doc.line([text("  "), member(class, field), text(" = "), text(value.clone())]);
        }
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::{Span, snippet};

    #[test]
    fn marks_matches_but_not_shift_operators() {
        let spans = snippet("int a = b >> 2; float >>radius<<() ... x <<= 1");
        assert_eq!(spans, [Span::Text("int a = b >> 2; float ".into()), Span::Mark("radius".into()), Span::Text("() ... x <<= 1".into())]);
    }
}
