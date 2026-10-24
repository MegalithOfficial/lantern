//! What the user picked, shared by the UI, the CLI and the MCP server. One JSON file in the
//! data folder. Unknown and missing fields are fine, an older or newer file still loads.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// The version whose index is open, by its folder under `versions/`. Empty for none.
    pub version: String,
    /// The first run setup went through, or was not needed.
    pub onboarded: bool,
    /// Id of the embedding model, empty for the default.
    pub embedding_model: String,
    /// Id of the reranker. Empty means the default when it is downloaded, "off" means none.
    pub reranker: String,
    /// Threads for embedding and reranking, 0 picks three quarters of the cores.
    pub threads: usize,
    /// Results a question returns.
    pub results: usize,
    /// Candidates the reranker reads. More is slower and finds a little more.
    pub rerank_pool: usize,
    pub chat: Chat,
}

/// A place that serves the OpenAI chat completions API.
#[derive(Debug)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    /// A model that can call tools, as an example of how this provider names them.
    pub example_model: &'static str,
    /// Whether `stream_options.include_usage` is understood. Some reject fields they do not know.
    pub stream_usage: bool,
    pub note: &'static str,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "nvidia",
        name: "NVIDIA NIM",
        base_url: "https://integrate.api.nvidia.com/v1",
        example_model: "nvidia/llama-3.3-nemotron-super-49b-v1.5",
        stream_usage: true,
        note: "Keys from build.nvidia.com. Pick a model whose page lists tool calling, not all of them have it.",
    },
    Provider {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        example_model: "anthropic/claude-sonnet-5",
        stream_usage: true,
        note: "One key for most models. Reports cost, and Claude and Gemini models get their prompt cache markers here.",
    },
    Provider {
        id: "nanogpt",
        name: "NanoGPT",
        base_url: "https://nano-gpt.com/api/v1",
        example_model: "z-ai/glm-5.3-flash",
        stream_usage: true,
        note: "Pay per use, one key for several hundred models. Load models lists them, a :thinking suffix turns reasoning on.",
    },
    Provider {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        example_model: "gpt-5.1",
        stream_usage: true,
        note: "Caches long prompts on its own.",
    },
    Provider {
        id: "anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com/v1",
        example_model: "claude-sonnet-5",
        stream_usage: true,
        note: "Through the OpenAI compatible endpoint, which has no prompt caching. Through OpenRouter the same models are cached.",
    },
    Provider {
        id: "google",
        name: "Google Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        example_model: "gemini-3-pro",
        stream_usage: true,
        note: "Keys from AI Studio. Caches long prompts on its own.",
    },
    Provider {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        example_model: "deepseek-chat",
        stream_usage: true,
        note: "Caches by prefix on its own and bills cached tokens at a fraction.",
    },
    Provider {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        example_model: "openai/gpt-oss-120b",
        stream_usage: true,
        note: "Very fast. Low rate limits on the free tier, long tool conversations run into them.",
    },
    Provider { id: "mistral", name: "Mistral", base_url: "https://api.mistral.ai/v1", example_model: "mistral-large-latest", stream_usage: false, note: "" },
    Provider { id: "xai", name: "xAI", base_url: "https://api.x.ai/v1", example_model: "grok-4", stream_usage: true, note: "Caches long prompts on its own." },
    Provider {
        id: "together",
        name: "Together AI",
        base_url: "https://api.together.xyz/v1",
        example_model: "Qwen/Qwen3-Coder-480B-A35B-Instruct-FP8",
        stream_usage: true,
        note: "",
    },
    Provider {
        id: "ollama",
        name: "Ollama (local)",
        base_url: "http://localhost:11434/v1",
        example_model: "qwen3:14b",
        stream_usage: true,
        note: "Runs on this machine, no key. Needs a model that can call tools, and a context of 16k or more to hold tool results.",
    },
    Provider {
        id: "lmstudio",
        name: "LM Studio (local)",
        base_url: "http://localhost:1234/v1",
        example_model: "qwen/qwen3-14b",
        stream_usage: true,
        note: "Runs on this machine, no key. Start the local server in LM Studio first.",
    },
    Provider {
        id: "custom",
        name: "Custom",
        base_url: "",
        example_model: "",
        stream_usage: true,
        note: "Any server that speaks the OpenAI chat completions API with tool calls: vLLM, llama.cpp, LiteLLM and the like.",
    },
];

pub fn provider(id: &str) -> &'static Provider {
    PROVIDERS.iter().find(|p| p.id == id).unwrap_or(&PROVIDERS[0])
}

/// What is kept per provider, so switching back and forth loses nothing.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Account {
    pub api_key: String,
    pub model: String,
    /// Only when it differs from the address of the provider, always for Custom.
    pub base_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Chat {
    pub provider: String,
    /// Keys are kept in the settings file, readable by this user only.
    pub accounts: BTreeMap<String, Account>,
    /// How many times the model may call tools before it has to answer.
    pub tool_rounds: usize,
    // From before there were providers, moved into `accounts` on load.
    #[serde(skip_serializing)]
    base_url: String,
    #[serde(skip_serializing)]
    api_key: String,
    #[serde(skip_serializing)]
    model: String,
}

/// Everything a request needs, resolved from the chosen provider and its account.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub provider: &'static Provider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: String::new(),
            onboarded: false,
            embedding_model: String::new(),
            reranker: String::new(),
            threads: 0,
            results: 20,
            rerank_pool: 40,
            chat: Chat::default(),
        }
    }
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            provider: PROVIDERS[0].id.to_owned(),
            accounts: BTreeMap::new(),
            tool_rounds: 12,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
        }
    }
}

impl Chat {
    pub fn account(&self) -> Account {
        self.accounts.get(&self.provider).cloned().unwrap_or_default()
    }

    /// `LANTERN_API_KEY` wins over the stored key.
    pub fn endpoint(&self) -> Endpoint {
        let provider = provider(&self.provider);
        let account = self.account();
        Endpoint {
            provider,
            base_url: if account.base_url.is_empty() { provider.base_url.to_owned() } else { account.base_url },
            api_key: std::env::var("LANTERN_API_KEY").ok().filter(|k| !k.is_empty()).unwrap_or(account.api_key),
            model: account.model.trim().to_owned(),
        }
    }

    fn migrate(&mut self) {
        if self.api_key.is_empty() && self.model.is_empty() {
            return;
        }
        let known = PROVIDERS.iter().find(|p| !p.base_url.is_empty() && p.base_url == self.base_url.trim_end_matches('/'));
        let id = known.map_or("custom", |p| p.id);
        let account = Account {
            api_key: std::mem::take(&mut self.api_key),
            model: std::mem::take(&mut self.model),
            base_url: if known.is_some() { String::new() } else { std::mem::take(&mut self.base_url) },
        };
        self.accounts.entry(id.to_owned()).or_insert(account);
        self.provider = id.to_owned();
        self.base_url.clear();
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

impl Settings {
    /// A missing or broken file gives the defaults, settings are never a reason not to start.
    pub fn load(data_dir: &Path) -> Self {
        let mut settings: Self = std::fs::read_to_string(path(data_dir)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        settings.chat.migrate();
        settings
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let target = path(data_dir);
        // Written next to the target and renamed over it, a crash never leaves half a file.
        let temporary = target.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", temporary.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temporary, &target).with_context(|| format!("writing {}", target.display()))?;
        Ok(())
    }
}
