//! Builds the index in two passes. The first reads every class file into memory, the second
//! gives every symbol an id and resolves references the way the JVM would, which can only be
//! done once all classes are known.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use cafebabe::{
    ClassAccessFlags, ClassFile,
    attributes::{AttributeData, AttributeInfo, BootstrapMethodEntry},
    bytecode::Opcode,
    constant_pool::{BootstrapArgument, LiteralConstant, Loadable, MemberKind},
};
use rusqlite::{Connection, params};

use crate::source::method_blocks;

const ACC_PRIVATE: u16 = 0x0002;
const ACC_STATIC: u16 = 0x0008;
const ACC_BRIDGE: u16 = 0x0040;
const ACC_SYNTHETIC: u16 = 0x1000;

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub classes: usize,
    pub external_classes: usize,
    pub methods: usize,
    pub calls: usize,
    pub unresolved_calls: usize,
    pub overrides: usize,
    pub key_fields: usize,
    pub method_sources: usize,
    pub data_files: usize,
    pub skipped: Vec<(String, String)>,
}

/// The parts of a build, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Classes,
    References,
    Symbols,
    Sources,
    Data,
    Finishing,
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::Classes => "Reading classes",
            Step::References => "Resolving references",
            Step::Symbols => "Writing symbols",
            Step::Sources => "Cutting sources into methods",
            Step::Data => "Reading data files",
            Step::Finishing => "Writing the index",
        }
    }

    /// What `done` and `total` count.
    pub fn unit(self) -> &'static str {
        match self {
            Step::Classes => "jar entries",
            Step::References => "classes",
            Step::Symbols => "symbols",
            Step::Sources => "source files",
            Step::Data => "files",
            Step::Finishing => "",
        }
    }
}

/// How far a build is. `total` is 0 where it is not known.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub step: Step,
    pub done: usize,
    pub total: usize,
}

type Report<'a> = &'a mut dyn FnMut(Progress);

struct CallRef {
    class: String,
    name: String,
    descriptor: String,
    kind: &'static str,
    line: Option<u16>,
}

struct FieldRef {
    class: String,
    name: String,
    descriptor: String,
    is_write: bool,
    line: Option<u16>,
}

/// What a static initializer does with ids, in order. Enough to tell which field holds which key.
enum KeyOp {
    Text(String),
    Get { class: String, field: String },
    Put { field: String },
}

struct MethodData {
    name: String,
    descriptor: String,
    flags: u16,
    line_start: Option<u16>,
    line_end: Option<u16>,
    calls: Vec<CallRef>,
    field_refs: Vec<FieldRef>,
    strings: Vec<(String, Option<u16>)>,
    key_ops: Vec<KeyOp>,
}

struct FieldData {
    name: String,
    descriptor: String,
    flags: u16,
    constant: Option<String>,
}

struct ClassData {
    name: String,
    super_name: Option<String>,
    interfaces: Vec<String>,
    flags: u16,
    is_interface: bool,
    source_file: Option<String>,
    fields: Vec<FieldData>,
    methods: Vec<MethodData>,
}

fn read_jar(jar: &Path, stats: &mut Stats, report: Report) -> Result<Vec<ClassData>> {
    let file = std::fs::File::open(jar).with_context(|| format!("opening {}", jar.display()))?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut classes = Vec::new();
    let mut bytes = Vec::new();
    let total = archive.len();
    for i in 0..total {
        report(Progress { step: Step::Classes, done: i, total });
        let mut entry = archive.by_index(i)?;
        if !entry.name().ends_with(".class") {
            continue;
        }
        let entry_name = entry.name().to_owned();
        bytes.clear();
        entry.read_to_end(&mut bytes)?;
        match cafebabe::parse_class(&bytes) {
            Ok(class) => classes.push(read_class(&class)),
            Err(err) => stats.skipped.push((entry_name, err.to_string())),
        }
    }
    Ok(classes)
}

fn read_class(class: &ClassFile) -> ClassData {
    let bootstrap = class.attributes.iter().find_map(|a| match &a.data {
        AttributeData::BootstrapMethods(entries) => Some(entries.as_slice()),
        _ => None,
    });

    let fields = class
        .fields
        .iter()
        .map(|field| FieldData {
            name: field.name.to_string(),
            descriptor: field.descriptor.to_string(),
            flags: field.access_flags.bits(),
            constant: field.attributes.iter().find_map(|a| match &a.data {
                AttributeData::ConstantValue(value) => Some(literal_to_string(value)),
                _ => None,
            }),
        })
        .collect();

    let methods = class.methods.iter().map(|method| read_method(method, bootstrap)).collect();

    ClassData {
        name: class.this_class.to_string(),
        super_name: class.super_class.as_ref().map(ToString::to_string),
        interfaces: class.interfaces.iter().map(ToString::to_string).collect(),
        flags: class.access_flags.bits(),
        is_interface: class.access_flags.contains(ClassAccessFlags::INTERFACE),
        source_file: class.attributes.iter().find_map(|a| match &a.data {
            AttributeData::SourceFile(s) => Some(s.to_string()),
            _ => None,
        }),
        fields,
        methods,
    }
}

fn read_method(method: &cafebabe::MethodInfo, bootstrap: Option<&[BootstrapMethodEntry]>) -> MethodData {
    let code = method.attributes.iter().find_map(|a| match &a.data {
        AttributeData::Code(code) => Some(code),
        _ => None,
    });
    let lines = code.map(|c| LineTable::new(&c.attributes)).unwrap_or_default();
    let mut data = MethodData {
        name: method.name.to_string(),
        descriptor: method.descriptor.to_string(),
        flags: method.access_flags.bits(),
        line_start: lines.first(),
        line_end: lines.last(),
        calls: Vec::new(),
        field_refs: Vec::new(),
        strings: Vec::new(),
        key_ops: Vec::new(),
    };
    let Some(bytecode) = code.and_then(|c| c.bytecode.as_ref()) else {
        return data;
    };
    let track_keys = data.name == "<clinit>";

    for (offset, opcode) in &bytecode.opcodes {
        let line = lines.at(*offset);
        let mut call = |target: &cafebabe::constant_pool::MemberRef, kind| {
            data.calls.push(CallRef {
                class: target.class_name.to_string(),
                name: target.name_and_type.name.to_string(),
                descriptor: target.name_and_type.descriptor.to_string(),
                kind,
                line,
            });
        };
        match opcode {
            Opcode::Invokevirtual(m) => call(m, "virtual"),
            Opcode::Invokestatic(m) => call(m, "static"),
            Opcode::Invokespecial(m) => call(m, "special"),
            Opcode::Invokeinterface(m, _) => call(m, "interface"),
            // A lambda or method reference is a method handle in the bootstrap arguments,
            // without this `forEach(Villager::stopSleeping)` would be invisible.
            Opcode::Invokedynamic(dynamic) => {
                let entry = bootstrap.and_then(|b| b.get(dynamic.attr_index as usize));
                for argument in entry.into_iter().flat_map(|e| &e.arguments) {
                    // Records pass field handles to their bootstrap method, those are not calls.
                    if let BootstrapArgument::MethodHandle(handle) = argument
                        && !matches!(handle.member_kind, MemberKind::Field)
                    {
                        data.calls.push(CallRef {
                            class: handle.class_name.to_string(),
                            name: handle.member_ref.name.to_string(),
                            descriptor: handle.member_ref.descriptor.to_string(),
                            kind: "lambda",
                            line,
                        });
                    }
                }
            }
            Opcode::Getfield(f) | Opcode::Getstatic(f) | Opcode::Putfield(f) | Opcode::Putstatic(f) => {
                let is_write = matches!(opcode, Opcode::Putfield(_) | Opcode::Putstatic(_));
                data.field_refs.push(FieldRef {
                    class: f.class_name.to_string(),
                    name: f.name_and_type.name.to_string(),
                    descriptor: f.name_and_type.descriptor.to_string(),
                    is_write,
                    line,
                });
                if track_keys {
                    match opcode {
                        Opcode::Getstatic(_) => data.key_ops.push(KeyOp::Get { class: f.class_name.to_string(), field: f.name_and_type.name.to_string() }),
                        Opcode::Putstatic(_) => data.key_ops.push(KeyOp::Put { field: f.name_and_type.name.to_string() }),
                        _ => {}
                    }
                }
            }
            Opcode::Ldc(loadable) | Opcode::LdcW(loadable) => {
                if let Loadable::LiteralConstant(LiteralConstant::String(value)) = loadable {
                    data.strings.push((value.to_string(), line));
                    if track_keys {
                        data.key_ops.push(KeyOp::Text(value.to_string()));
                    }
                }
            }
            _ => {}
        }
    }
    data
}

fn literal_to_string(value: &LiteralConstant) -> String {
    match value {
        LiteralConstant::Integer(v) => v.to_string(),
        LiteralConstant::Float(v) => v.to_string(),
        LiteralConstant::Long(v) => v.to_string(),
        LiteralConstant::Double(v) => v.to_string(),
        LiteralConstant::String(v) => v.to_string(),
        LiteralConstant::StringBytes(v) => String::from_utf8_lossy(v).into_owned(),
    }
}

#[derive(Default)]
struct LineTable {
    entries: Vec<(u16, u16)>,
}

impl LineTable {
    fn new(attributes: &[AttributeInfo]) -> Self {
        let mut entries: Vec<(u16, u16)> = attributes
            .iter()
            .filter_map(|a| match &a.data {
                AttributeData::LineNumberTable(table) => Some(table),
                _ => None,
            })
            .flatten()
            .map(|e| (e.start_pc, e.line_number))
            .collect();
        entries.sort_unstable();
        Self { entries }
    }

    fn at(&self, offset: usize) -> Option<u16> {
        let index = self.entries.partition_point(|(pc, _)| usize::from(*pc) <= offset);
        index.checked_sub(1).map(|i| self.entries[i].1)
    }

    fn first(&self) -> Option<u16> {
        self.entries.iter().map(|(_, line)| *line).min()
    }

    fn last(&self) -> Option<u16> {
        self.entries.iter().map(|(_, line)| *line).max()
    }
}

struct ClassRow {
    name: String,
    super_id: Option<i64>,
    interface_ids: Vec<i64>,
    is_external: bool,
}

struct MethodRow {
    class_id: i64,
    name: String,
    descriptor: String,
    flags: u16,
    is_external: bool,
}

/// Ids start at 1 and are positions in these vectors plus one.
#[derive(Default)]
struct Symbols {
    classes: Vec<ClassRow>,
    class_ids: HashMap<String, i64>,
    methods: Vec<MethodRow>,
    method_ids: HashMap<(i64, String, String), i64>,
    field_ids: HashMap<(i64, String), i64>,
    fields: Vec<(i64, String, String, u16, bool, Option<String>)>,
}

impl Symbols {
    fn class(&mut self, name: &str) -> i64 {
        if let Some(id) = self.class_ids.get(name) {
            return *id;
        }
        self.classes.push(ClassRow { name: name.to_owned(), super_id: None, interface_ids: Vec::new(), is_external: true });
        let id = self.classes.len() as i64;
        self.class_ids.insert(name.to_owned(), id);
        id
    }

    fn add_method(&mut self, class_id: i64, name: &str, descriptor: &str, flags: u16, is_external: bool) -> i64 {
        let key = (class_id, name.to_owned(), descriptor.to_owned());
        if let Some(id) = self.method_ids.get(&key) {
            return *id;
        }
        self.methods.push(MethodRow { class_id, name: key.1.clone(), descriptor: key.2.clone(), flags, is_external });
        let id = self.methods.len() as i64;
        self.method_ids.insert(key, id);
        id
    }

    fn add_field(&mut self, class_id: i64, name: &str, descriptor: &str, flags: u16, is_external: bool, constant: Option<String>) -> i64 {
        let key = (class_id, name.to_owned());
        if let Some(id) = self.field_ids.get(&key) {
            return *id;
        }
        self.fields.push((class_id, name.to_owned(), descriptor.to_owned(), flags, is_external, constant));
        let id = self.fields.len() as i64;
        self.field_ids.insert(key, id);
        id
    }

    /// Super class first, then interfaces, breadth first. That is the order the JVM looks a
    /// member up in, close enough for everything that compiles.
    fn lookup_order(&self, class_id: i64) -> Vec<i64> {
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([class_id]);
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            order.push(id);
            let row = &self.classes[id as usize - 1];
            queue.extend(row.super_id);
            queue.extend(row.interface_ids.iter().copied());
        }
        order
    }

    fn resolve_method(&self, class_id: i64, name: &str, descriptor: &str) -> Option<i64> {
        let key_name = name.to_owned();
        let key_desc = descriptor.to_owned();
        self.lookup_order(class_id).into_iter().find_map(|id| self.method_ids.get(&(id, key_name.clone(), key_desc.clone())).copied())
    }

    fn resolve_field(&self, class_id: i64, name: &str) -> Option<i64> {
        let key_name = name.to_owned();
        self.lookup_order(class_id).into_iter().find_map(|id| self.field_ids.get(&(id, key_name.clone())).copied())
    }
}

fn split_name(name: &str) -> (&str, &str) {
    name.rsplit_once('/').unwrap_or(("", name))
}

fn outer_name(name: &str) -> Option<&str> {
    name.rsplit_once('$').map(|(outer, _)| outer)
}

fn is_ident(value: &str) -> bool {
    !value.is_empty()
        && value.len() < 96
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'/' | b'.' | b':' | b'-'))
        && value.bytes().any(|b| b.is_ascii_lowercase())
}

/// What an index is built from. Only the jar is required, without sources there is no
/// decompiled text and without data no tags or registries.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub version: String,
    pub jar: PathBuf,
    pub src: Option<PathBuf>,
    pub data: Option<PathBuf>,
}

/// Writes a fresh index to `db_path`. It is built next to it and moved over it at the end, a
/// failed build leaves the old one as it was and a running one is never mistaken for an
/// index. Nothing may have the old one open, on Windows the file can not be replaced otherwise.
pub fn build_file(db_path: &Path, inputs: &Inputs, report: Report) -> Result<Stats> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = db_path.with_extension("db.part");
    let stats = build_into(&partial, inputs, report);
    if stats.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    let stats = stats?;
    std::fs::rename(&partial, db_path).with_context(|| format!("replacing {}", db_path.display()))?;
    Ok(stats)
}

fn build_into(db_path: &Path, inputs: &Inputs, report: Report) -> Result<Stats> {
    let mut conn = crate::db::create(db_path)?;
    let stats = build_with(&mut conn, &inputs.jar, inputs.data.as_deref(), inputs.src.as_deref(), report)?;

    // Not canonicalize, that turns Windows paths into the \\?\ form nothing else likes.
    let absolute = |p: &PathBuf| std::path::absolute(p).unwrap_or_else(|_| p.clone()).display().to_string();
    let mut meta = vec![("version", inputs.version.clone()), ("jar", absolute(&inputs.jar))];
    meta.extend(inputs.src.as_ref().map(|p| ("src_root", absolute(p))));
    meta.extend(inputs.data.as_ref().map(|p| ("data_root", absolute(p))));
    set_meta(&conn, &meta)?;
    report(Progress { step: Step::Finishing, done: 0, total: 0 });
    conn.execute_batch("ANALYZE;")?;
    Ok(stats)
}

pub fn build(conn: &mut Connection, jar: &Path, data_dir: Option<&Path>, src_dir: Option<&Path>) -> Result<Stats> {
    build_with(conn, jar, data_dir, src_dir, &mut |_| {})
}

pub fn build_with(conn: &mut Connection, jar: &Path, data_dir: Option<&Path>, src_dir: Option<&Path>, report: Report) -> Result<Stats> {
    let mut stats = Stats::default();
    let classes = read_jar(jar, &mut stats, report)?;
    stats.classes = classes.len();

    let mut symbols = Symbols::default();
    for class in &classes {
        let id = symbols.class(&class.name);
        symbols.classes[id as usize - 1].is_external = false;
    }
    for class in &classes {
        let id = symbols.class_ids[&class.name];
        let super_id = class.super_name.as_deref().map(|name| symbols.class(name));
        let interface_ids: Vec<i64> = class.interfaces.iter().map(|name| symbols.class(name)).collect();
        let row = &mut symbols.classes[id as usize - 1];
        row.super_id = super_id;
        row.interface_ids = interface_ids;

        for field in &class.fields {
            symbols.add_field(id, &field.name, &field.descriptor, field.flags, false, field.constant.clone());
        }
        for method in &class.methods {
            symbols.add_method(id, &method.name, &method.descriptor, method.flags, false);
        }
    }
    stats.methods = symbols.methods.len();

    let tx = conn.transaction()?;

    // References. Anything that does not resolve inside the jar becomes an external symbol
    // on the class that was named at the call site.
    {
        let mut insert_call = tx.prepare("INSERT INTO calls (caller_id, callee_id, receiver_id, kind, line) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        let mut insert_field_ref = tx.prepare("INSERT INTO field_refs (method_id, field_id, is_write, line) VALUES (?1, ?2, ?3, ?4)")?;
        let mut insert_string = tx.prepare("INSERT INTO strings (method_id, value, line) VALUES (?1, ?2, ?3)")?;
        let mut resolved: HashMap<(i64, String, String), i64> = HashMap::new();

        for (done, class) in classes.iter().enumerate() {
            report(Progress { step: Step::References, done, total: classes.len() });
            let class_id = symbols.class_ids[&class.name];
            for method in &class.methods {
                let caller = symbols.method_ids[&(class_id, method.name.clone(), method.descriptor.clone())];
                for call in &method.calls {
                    let receiver = symbols.class(&call.class);
                    let key = (receiver, call.name.clone(), call.descriptor.clone());
                    let callee = match resolved.get(&key) {
                        Some(id) => *id,
                        None => {
                            let id = symbols.resolve_method(receiver, &call.name, &call.descriptor).unwrap_or_else(|| {
                                if !symbols.classes[receiver as usize - 1].is_external {
                                    stats.unresolved_calls += 1;
                                }
                                symbols.add_method(receiver, &call.name, &call.descriptor, 0, true)
                            });
                            resolved.insert(key, id);
                            id
                        }
                    };
                    insert_call.execute(params![caller, callee, receiver, call.kind, call.line])?;
                    stats.calls += 1;
                }
                for field_ref in &method.field_refs {
                    let owner = symbols.class(&field_ref.class);
                    let field = symbols
                        .resolve_field(owner, &field_ref.name)
                        .unwrap_or_else(|| symbols.add_field(owner, &field_ref.name, &field_ref.descriptor, 0, true, None));
                    insert_field_ref.execute(params![caller, field, field_ref.is_write, field_ref.line])?;
                }
                for (value, line) in &method.strings {
                    insert_string.execute(params![caller, value, line])?;
                }
            }
        }
    }

    // Classes, with everything that got interned along the way.
    report(Progress { step: Step::Symbols, done: 0, total: 0 });
    {
        let mut insert = tx.prepare(
            "INSERT INTO classes (id, name, simple, package, outer_id, super_id, flags, is_interface, is_external, source_file)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        let mut insert_interface = tx.prepare("INSERT OR IGNORE INTO interfaces (class_id, interface_id) VALUES (?1, ?2)")?;
        let by_name: HashMap<&str, &ClassData> = classes.iter().map(|c| (c.name.as_str(), c)).collect();
        for (index, row) in symbols.classes.iter().enumerate() {
            let id = index as i64 + 1;
            let (package, simple) = split_name(&row.name);
            let data = by_name.get(row.name.as_str());
            let outer_id = outer_name(&row.name).and_then(|outer| symbols.class_ids.get(outer)).copied();
            insert.execute(params![
                id,
                row.name,
                simple,
                package,
                outer_id,
                row.super_id,
                data.map_or(0, |d| d.flags),
                data.is_some_and(|d| d.is_interface),
                row.is_external,
                data.and_then(|d| d.source_file.clone()),
            ])?;
            for interface_id in &row.interface_ids {
                insert_interface.execute(params![id, interface_id])?;
            }
        }
        stats.external_classes = symbols.classes.iter().filter(|c| c.is_external).count();
    }

    // Closure over extends and implements.
    {
        let mut insert = tx.prepare("INSERT OR IGNORE INTO ancestors (class_id, ancestor_id, depth) VALUES (?1, ?2, ?3)")?;
        for index in 0..symbols.classes.len() {
            let id = index as i64 + 1;
            let mut depth_of: BTreeMap<i64, i64> = BTreeMap::new();
            let mut queue = VecDeque::from([(id, 0i64)]);
            while let Some((current, depth)) = queue.pop_front() {
                if depth_of.contains_key(&current) {
                    continue;
                }
                depth_of.insert(current, depth);
                let row = &symbols.classes[current as usize - 1];
                queue.extend(row.super_id.map(|s| (s, depth + 1)));
                queue.extend(row.interface_ids.iter().map(|i| (*i, depth + 1)));
            }
            for (ancestor, depth) in depth_of {
                insert.execute(params![id, ancestor, depth])?;
            }
        }
    }

    // Methods, with the declaration each one overrides.
    {
        let line_info: HashMap<(i64, &str, &str), (Option<u16>, Option<u16>)> = classes
            .iter()
            .flat_map(|class| {
                let class_id = symbols.class_ids[&class.name];
                class.methods.iter().map(move |m| ((class_id, m.name.as_str(), m.descriptor.as_str()), (m.line_start, m.line_end)))
            })
            .collect();

        let mut insert = tx.prepare(
            "INSERT INTO methods (id, class_id, name, descriptor, flags, is_external, line_start, line_end, overrides_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for (index, method) in symbols.methods.iter().enumerate() {
            let id = index as i64 + 1;
            let can_override = !method.is_external && method.flags & (ACC_PRIVATE | ACC_STATIC) == 0 && !method.name.starts_with('<');
            let overrides = can_override
                .then(|| {
                    symbols.lookup_order(method.class_id).into_iter().skip(1).find_map(|ancestor| {
                        let candidate = *symbols.method_ids.get(&(ancestor, method.name.clone(), method.descriptor.clone()))?;
                        let flags = symbols.methods[candidate as usize - 1].flags;
                        (flags & (ACC_PRIVATE | ACC_STATIC) == 0).then_some(candidate)
                    })
                })
                .flatten();
            stats.overrides += usize::from(overrides.is_some());
            let (line_start, line_end) = line_info.get(&(method.class_id, method.name.as_str(), method.descriptor.as_str())).copied().unwrap_or_default();
            insert.execute(params![id, method.class_id, method.name, method.descriptor, method.flags, method.is_external, line_start, line_end, overrides])?;
        }
    }

    {
        let mut insert = tx.prepare("INSERT INTO fields (id, class_id, name, descriptor, flags, is_external, constant) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for (index, (class_id, name, descriptor, flags, is_external, constant)) in symbols.fields.iter().enumerate() {
            insert.execute(params![index as i64 + 1, class_id, name, descriptor, flags, is_external, constant])?;
        }
    }

    stats.key_fields = index_key_fields(&tx, &classes, &symbols)?;
    if let Some(src_dir) = src_dir {
        stats.method_sources = index_sources(&tx, &classes, &symbols, src_dir, report)?;
    }
    if let Some(data_dir) = data_dir {
        stats.data_files = index_data(&tx, data_dir, report)?;
    }

    report(Progress { step: Step::Finishing, done: 0, total: 0 });
    tx.commit()?;
    Ok(stats)
}

/// `BlockTags.WOOL = create("wool")` stores the id loaded last before the `putstatic`. A field
/// that is filled from another key field (`BlockTags.X = BlockItemTags.X.block()`) inherits
/// that field's id, which takes a second round because the other class may come later.
fn index_key_fields(tx: &Connection, classes: &[ClassData], symbols: &Symbols) -> Result<usize> {
    let mut direct: HashMap<i64, String> = HashMap::new();
    let mut aliases: Vec<(i64, i64)> = Vec::new();

    for class in classes {
        if !class.name.starts_with("net/minecraft/") && !class.name.starts_with("com/mojang/") {
            continue;
        }
        let class_id = symbols.class_ids[&class.name];
        let Some(clinit) = class.methods.iter().find(|m| m.name == "<clinit>") else { continue };

        let mut last_text: Option<&str> = None;
        let mut gets: Vec<i64> = Vec::new();
        for op in &clinit.key_ops {
            match op {
                KeyOp::Text(value) => last_text = Some(value),
                KeyOp::Get { class, field } => {
                    gets.extend(symbols.class_ids.get(class).and_then(|id| symbols.resolve_field(*id, field)));
                }
                KeyOp::Put { field } => {
                    if let Some(field_id) = symbols.field_ids.get(&(class_id, field.clone())).copied() {
                        match (last_text.filter(|t| is_ident(t)), gets.as_slice()) {
                            (Some(text), _) => {
                                direct.insert(field_id, text.to_owned());
                            }
                            // Exactly one other field went into this one. With more than one
                            // (an enum constant built from several sounds) there is no telling.
                            (None, [source]) if *source != field_id => aliases.push((field_id, *source)),
                            _ => {}
                        }
                    }
                    last_text = None;
                    gets.clear();
                }
            }
        }
    }

    let mut via: HashMap<i64, i64> = HashMap::new();
    loop {
        let mut changed = false;
        for (field_id, source) in &aliases {
            if direct.contains_key(field_id) {
                continue;
            }
            if let Some(ident) = direct.get(source).cloned() {
                direct.insert(*field_id, ident);
                via.insert(*field_id, *source);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut insert = tx.prepare("INSERT INTO key_fields (field_id, ident, via_id) VALUES (?1, ?2, ?3)")?;
    for (field_id, ident) in &direct {
        insert.execute(params![field_id, ident, via.get(field_id)])?;
    }
    Ok(direct.len())
}

fn index_sources(tx: &Connection, classes: &[ClassData], symbols: &Symbols, src_dir: &Path, report: Report) -> Result<usize> {
    // Methods that can appear in a source file, by the outermost class they live in.
    let mut by_outer: HashMap<&str, Vec<(&ClassData, &MethodData)>> = HashMap::new();
    for class in classes {
        let outer = class.name.split('$').next().unwrap_or(&class.name);
        for method in &class.methods {
            let hidden = method.flags & (ACC_SYNTHETIC | ACC_BRIDGE) != 0 || method.name == "<clinit>" || method.name.contains('$');
            if !hidden {
                by_outer.entry(outer).or_default().push((class, method));
            }
        }
    }

    let mut insert_class = tx.prepare("INSERT INTO class_sources (class_id, path, text) VALUES (?1, ?2, ?3)")?;
    let mut insert_class_fts = tx.prepare("INSERT INTO class_fts (rowid, title, body) VALUES (?1, ?2, ?3)")?;
    let mut insert_method = tx.prepare("INSERT OR IGNORE INTO method_sources (method_id, line, text) VALUES (?1, ?2, ?3)")?;
    let mut insert_method_fts = tx.prepare("INSERT INTO method_fts (rowid, title, body) VALUES (?1, ?2, ?3)")?;
    let mut count = 0;

    // Listed first, the count is what the progress is measured against.
    let files: Vec<_> = walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("java"))
        .collect();
    for (done, entry) in files.iter().enumerate() {
        report(Progress { step: Step::Sources, done, total: files.len() });
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(src_dir) else { continue };
        let class_name = relative.with_extension("").to_string_lossy().replace('\\', "/");
        let Some(class_id) = symbols.class_ids.get(&class_name).copied() else { continue };
        let body = std::fs::read_to_string(path)?;
        let simple = split_name(&class_name).1;

        insert_class.execute(params![class_id, relative.to_string_lossy().replace('\\', "/"), body])?;
        insert_class_fts.execute(params![class_id, format!("{simple} {}", humanize(simple)), body])?;

        let Some(methods) = by_outer.get(class_name.as_str()) else { continue };
        let mut by_source_name: BTreeMap<String, Vec<(&ClassData, &MethodData)>> = BTreeMap::new();
        for (class, method) in methods {
            let source_name = if method.name == "<init>" {
                let own = split_name(&class.name).1;
                own.rsplit('$').next().unwrap_or(own).to_owned()
            } else {
                method.name.clone()
            };
            by_source_name.entry(source_name).or_default().push((class, method));
        }

        for (source_name, mut declared) in by_source_name {
            // Both lists are in source order, overloads pair up one to one.
            declared.sort_by_key(|(_, m)| m.line_start.unwrap_or(u16::MAX));
            for ((class, method), (line, text)) in declared.into_iter().zip(method_blocks(&body, &source_name)) {
                let method_class = symbols.class_ids[&class.name];
                let method_id = symbols.method_ids[&(method_class, method.name.clone(), method.descriptor.clone())];
                if insert_method.execute(params![method_id, line as i64, text])? == 1 {
                    let own = split_name(&class.name).1;
                    let title = format!("{own}.{source_name} {} {}", humanize(own), humanize(&source_name));
                    insert_method_fts.execute(params![method_id, title, text])?;
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

/// `tryToWarn` -> `try to warn`, so that a question in plain English has words to match.
pub fn humanize(identifier: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for ch in identifier.chars() {
        if ch == '_' || ch == '$' {
            out.push(' ');
            previous_lower = false;
            continue;
        }
        if ch.is_uppercase() && previous_lower {
            out.push(' ');
        }
        previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
        out.extend(ch.to_lowercase());
    }
    out
}

fn index_data(tx: &Connection, data_dir: &Path, report: Report) -> Result<usize> {
    let mut insert = tx.prepare("INSERT OR IGNORE INTO data_files (registry, ident, path) VALUES (?1, ?2, ?3)")?;
    let mut insert_member = tx.prepare("INSERT OR IGNORE INTO tag_members (tag_id, member) VALUES (?1, ?2)")?;
    let mut count = 0;

    let files: Vec<_> = walkdir::WalkDir::new(data_dir).sort_by_file_name().into_iter().filter_map(Result::ok).filter(|e| e.file_type().is_file()).collect();
    for (done, entry) in files.iter().enumerate() {
        report(Progress { step: Step::Data, done, total: files.len() });
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(data_dir) else { continue };
        let mut parts: Vec<&str> = relative.iter().filter_map(|p| p.to_str()).collect();
        if parts.len() < 3 {
            continue;
        }
        let namespace = parts.remove(0);
        let file = parts.pop().unwrap_or_default();
        let Some((stem, extension)) = file.rsplit_once('.') else { continue };

        // tags/block/mineable/axe.json is registry tags/block and id mineable/axe. Worldgen
        // has one more level, everything else is a single folder.
        let registry_len = match parts.first().copied() {
            Some("tags") if parts.get(1) == Some(&"worldgen") => 3,
            Some("tags" | "worldgen") => 2,
            _ => 1,
        }
        .min(parts.len());
        let registry = parts[..registry_len].join("/");
        let mut ident_path = parts[registry_len..].to_vec();
        ident_path.push(stem);
        let ident = format!("{namespace}:{}", ident_path.join("/"));

        if insert.execute(params![registry, ident, relative.to_string_lossy().replace('\\', "/")])? == 0 {
            continue;
        }
        let file_id = tx.last_insert_rowid();
        count += 1;

        if registry.starts_with("tags/") && extension == "json" {
            let text = std::fs::read_to_string(path)?;
            let json: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            for member in json["values"].as_array().into_iter().flatten() {
                if let Some(member) = member.as_str().or_else(|| member["id"].as_str()) {
                    insert_member.execute(params![file_id, member])?;
                }
            }
        }
    }
    Ok(count)
}

pub fn set_meta(conn: &Connection, values: &[(&str, String)]) -> Result<()> {
    for (key, value) in values {
        conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value])?;
    }
    Ok(())
}
