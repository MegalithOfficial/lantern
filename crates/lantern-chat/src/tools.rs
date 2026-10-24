//! The tools Lantern offers to a model, the same set for the MCP server and the built in chat.

use std::path::{Path, PathBuf};

use anyhow::Result;
use lantern_core::{
    db::Connection,
    render::{self, Query},
};
use lantern_embed::Searcher;
use serde_json::{Value, json};

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// (argument, description, required)
    pub arguments: &'static [(&'static str, &'static str, bool)],
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "topic",
        description: "Start here. Everything connected to a topic in the vanilla server (e.g. \"warden anger\"): matching classes, who uses them, what they use, the tags and registry keys they touch, related data files and constants.",
        arguments: &[("seed", "Words that appear in the class names, e.g. \"sculk sensor\"", true), ("depth", "Hops through the call graph, default 1", false)],
    },
    Tool {
        name: "ask",
        description: "Ask in plain English, e.g. `how do villagers summon an iron golem`. Semantic search over every method and class merged with full text search, reranked by a cross encoder. Returns methods with file and line. Use when you know what the code does but not what it is called. The first call loads the models and takes a few seconds longer.",
        arguments: &[("question", "What you want to know", true), ("limit", "Max results, default 12", false)],
    },
    Tool {
        name: "class",
        description: "Super class, interfaces, subclasses, fields with constant values and methods of a class. Accepts a simple name like Warden or a full name.",
        arguments: &[("name", "Class name", true)],
    },
    Tool {
        name: "source",
        description: "Decompiled Java source of a class, or of a single method when `method` is given. Prefer passing a method, whole classes can be very long.",
        arguments: &[("class", "Class name", true), ("method", "Method name", false)],
    },
    Tool {
        name: "callers",
        description: "Who calls Class.method, including calls made through a super type, an interface or a method reference.",
        arguments: &[("target", "Class.method", true)],
    },
    Tool {
        name: "callees",
        description: "What Class.method calls inside the game code, in order, with line numbers.",
        arguments: &[("target", "Class.method", true)],
    },
    Tool { name: "refs", description: "Which methods read or write Class.FIELD.", arguments: &[("target", "Class.FIELD", true)] },
    Tool {
        name: "key",
        description: "Look up a tag or registry id such as dampens_vibrations or minecraft:shrieker_can_listen: the data file and its members, the Java constant that holds it and every method using that constant. Use this to tell whether a value is data driven or hardcoded.",
        arguments: &[("ident", "Tag or registry id, namespace optional", true)],
    },
    Tool {
        name: "strings",
        description: "Methods that load a string constant containing the pattern. Good for NBT keys, translation keys and ids.",
        arguments: &[("pattern", "Substring of the constant", true)],
    },
    Tool {
        name: "list_files",
        description: "List a folder of the decompiled tree. Paths are relative: `net/minecraft/world/entity` for code, `data/minecraft/loot_table` for the JSON data (tags, loot tables, recipes, worldgen, timelines ...). Start with an empty path to see the top.",
        arguments: &[("path", "Folder, empty for the top", false)],
    },
    Tool {
        name: "read_file",
        description: "Read a file of the decompiled tree with line numbers. Use it for data JSON, and for code when you want the surroundings of a method or a whole small class. Long files come in pieces, pass `from` to continue.",
        arguments: &[
            ("path", "File, e.g. data/minecraft/loot_table/chests/ancient_city.json", true),
            ("from", "First line, default 1", false),
            ("lines", "How many lines, default 300, at most 800", false),
        ],
    },
    Tool {
        name: "grep",
        description: "Regular expression search through the files themselves, code and data. Case insensitive unless the pattern has an upper case letter. Use it for what the index does not know: a JSON value across all loot tables (path `data`), a comment, a literal number, a local variable name. Returns path:line: text.",
        arguments: &[
            ("pattern", "Regular expression, or plain text", true),
            ("path", "Folder or file to search in, default everything. `data` for the JSON data only", false),
            ("extension", "Only files with this extension, e.g. java or json", false),
            ("limit", "Max lines shown, default 60", false),
        ],
    },
    Tool {
        name: "search",
        description: "Full text search over all decompiled sources (SQLite FTS5 syntax, e.g. `sleeping NEAR villager`). Use when you do not know the class name.",
        arguments: &[("query", "FTS5 query", true), ("limit", "Max results, default 15", false)],
    },
];

/// JSON schema of the arguments of a tool, the shape both MCP and the OpenAI API want.
pub fn schema(tool: &Tool) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, description, is_required) in tool.arguments {
        let kind = if matches!(*name, "depth" | "limit" | "from" | "lines") { "integer" } else { "string" };
        properties.insert((*name).to_owned(), json!({ "type": kind, "description": description }));
        if *is_required {
            required.push(*name);
        }
    }
    json!({ "type": "object", "properties": properties, "required": required })
}

fn query_for(name: &str, args: &Value) -> Result<Query> {
    let text = |key: &str| -> Result<String> { args[key].as_str().map(str::to_owned).ok_or_else(|| anyhow::anyhow!("missing argument `{key}`")) };
    let number = |key: &str, default: usize| args[key].as_u64().map_or(default, |n| n as usize);
    Ok(match name {
        "topic" => Query::Topic { seed: text("seed")?, depth: number("depth", 1) },
        "class" => Query::Class { name: text("name")? },
        "source" => Query::Source { class: text("class")?, method: args["method"].as_str().map(str::to_owned) },
        "callers" => Query::Callers { target: text("target")? },
        "callees" => Query::Callees { target: text("target")? },
        "refs" => Query::Refs { target: text("target")? },
        "key" => Query::Key { ident: text("ident")? },
        "strings" => Query::Strings { pattern: text("pattern")? },
        "search" => Query::Search { query: text("query")?, limit: number("limit", 15) },
        other => anyhow::bail!("unknown tool `{other}`"),
    })
}

/// An open index and, once something asked for it, the loaded models.
pub struct Toolbox {
    pub conn: Connection,
    data_dir: PathBuf,
    searcher: Option<Searcher>,
    files: crate::files::Roots,
}

impl Toolbox {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = lantern_core::db::open(db_path)?;
        let files = crate::files::Roots::new(lantern_core::query::meta(&conn, "src_root"), lantern_core::query::meta(&conn, "data_root"));
        Ok(Self { conn, data_dir: lantern_embed::data_dir(db_path), searcher: None, files })
    }

    /// Runs a tool and returns its text, cut to `max_chars`.
    pub fn run(&mut self, name: &str, args: &Value, max_chars: usize) -> Result<String> {
        let mut text = match name {
            "ask" => {
                let question = args["question"].as_str().filter(|q| !q.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("missing argument `question`"))?;
                let limit = args["limit"].as_u64().map_or(12, |n| n as usize).clamp(1, 40);
                if self.searcher.is_none() {
                    self.searcher = Some(lantern_embed::open_default(&self.conn, &self.data_dir)?);
                }
                let searcher = self.searcher.as_mut().expect("loaded right above");
                let answers = searcher.ask(&self.conn, question, limit)?;
                lantern_embed::render_answers(&self.conn, &answers)
            }
            "list_files" => self.files.list(args["path"].as_str().unwrap_or_default())?,
            "read_file" => {
                let path = args["path"].as_str().ok_or_else(|| anyhow::anyhow!("missing argument `path`"))?;
                let from = args["from"].as_u64().map_or(1, |n| n as usize);
                let lines = args["lines"].as_u64().map_or(300, |n| n as usize).clamp(1, 800);
                self.files.read(path, from, lines)?
            }
            "grep" => {
                let pattern = args["pattern"].as_str().filter(|p| !p.is_empty()).ok_or_else(|| anyhow::anyhow!("missing argument `pattern`"))?;
                let limit = args["limit"].as_u64().map_or(60, |n| n as usize).clamp(1, 300);
                self.files.grep(pattern, args["path"].as_str().unwrap_or_default(), args["extension"].as_str(), limit)?
            }
            name => render::run(&self.conn, &query_for(name, args)?)?,
        };
        if text.len() > max_chars {
            let mut cut = max_chars;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push_str("\n... truncated, narrow the query");
        }
        Ok(text)
    }
}
