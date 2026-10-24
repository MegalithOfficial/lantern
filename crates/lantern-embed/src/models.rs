#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pooling {
    /// The model already outputs one vector per input under this name.
    Output(&'static str),
    Mean,
    Cls,
    LastToken,
}

#[derive(Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub repo: &'static str,
    /// The graph, plus the files it needs next to it (external weights).
    pub onnx: &'static str,
    pub onnx_extra: &'static [&'static str],
    pub params_millions: u32,
    pub download_mb: u32,
    pub dims: usize,
    /// Vectors are cut to this many dimensions before they are stored. Only smaller than
    /// `dims` for models trained with Matryoshka loss, where the prefix is a valid embedding.
    pub stored_dims: usize,
    /// Hard limit of the model. Nothing longer than this is ever sent to it.
    pub context_tokens: usize,
    /// What a chunk is cut to. Far below the context on purpose: one method per vector
    /// searches better than half a class per vector.
    pub chunk_tokens: usize,
    pub pooling: Pooling,
    pub query_prefix: &'static str,
    /// `{title}` is replaced with the symbol name.
    pub document_prefix: &'static str,
    pub license: &'static str,
    pub note: &'static str,
}

pub const DEFAULT_MODEL: &str = "embeddinggemma-300m-q8";

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "embeddinggemma-300m-q8",
        name: "EmbeddingGemma 300M (8 bit)",
        repo: "onnx-community/embeddinggemma-300m-ONNX",
        onnx: "onnx/model_quantized.onnx",
        onnx_extra: &["onnx/model_quantized.onnx_data"],
        params_millions: 308,
        download_mb: 310,
        dims: 768,
        stored_dims: 256,
        context_tokens: 2048,
        chunk_tokens: 512,
        pooling: Pooling::Output("sentence_embedding"),
        query_prefix: "task: code retrieval | query: ",
        document_prefix: "title: {title} | text: ",
        license: "Gemma terms of use",
        note: "Best code retrieval score of the models under 500M parameters. The default.",
    },
    ModelSpec {
        id: "granite-small-r2-q8",
        name: "Granite Embedding Small R2 (8 bit)",
        repo: "onnx-community/granite-embedding-small-english-r2-ONNX",
        onnx: "onnx/model_quantized.onnx",
        onnx_extra: &["onnx/model_quantized.onnx_data"],
        params_millions: 47,
        download_mb: 50,
        dims: 384,
        stored_dims: 384,
        context_tokens: 8192,
        chunk_tokens: 512,
        pooling: Pooling::Cls,
        query_prefix: "",
        document_prefix: "",
        license: "Apache-2.0",
        note: "The light one. Six times smaller than the default, trained on code as well.",
    },
    ModelSpec {
        id: "jina-v5-nano-q8",
        name: "Jina Embeddings v5 Text Nano (8 bit)",
        repo: "jinaai/jina-embeddings-v5-text-nano-retrieval",
        onnx: "onnx/model_quantized.onnx",
        onnx_extra: &["onnx/model_quantized.onnx_data"],
        params_millions: 239,
        download_mb: 250,
        dims: 768,
        stored_dims: 256,
        context_tokens: 8192,
        chunk_tokens: 512,
        pooling: Pooling::LastToken,
        query_prefix: "Query: ",
        document_prefix: "Document: ",
        license: "CC BY-NC 4.0 (non commercial)",
        note: "Newest of the three, strongest on plain English. No code specific training.",
    },
];

/// Reads a question and a candidate together and scores the pair.
#[derive(Debug)]
pub struct RerankerSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub repo: &'static str,
    pub onnx: &'static str,
    pub params_millions: u32,
    pub download_mb: u32,
    pub context_tokens: usize,
    /// Question and candidate together are cut to this. Cost grows with length and the
    /// start of a method says most of what it is about.
    pub pair_tokens: usize,
    pub license: &'static str,
    pub note: &'static str,
}

pub const DEFAULT_RERANKER: &str = "gte-reranker-modernbert-q8";

pub const RERANKERS: &[RerankerSpec] = &[
    RerankerSpec {
        id: "gte-reranker-modernbert-q8",
        name: "GTE Reranker ModernBERT (8 bit)",
        repo: "Alibaba-NLP/gte-reranker-modernbert-base",
        onnx: "onnx/model_int8.onnx",
        params_millions: 149,
        download_mb: 151,
        context_tokens: 8192,
        pair_tokens: 1024,
        license: "Apache-2.0",
        note: "Trained on code as well as text, long context, fast for its quality. The default.",
    },
    RerankerSpec {
        id: "gte-reranker-modernbert",
        name: "GTE Reranker ModernBERT (full precision)",
        repo: "Alibaba-NLP/gte-reranker-modernbert-base",
        onnx: "onnx/model.onnx",
        params_millions: 149,
        download_mb: 599,
        context_tokens: 8192,
        pair_tokens: 1024,
        license: "Apache-2.0",
        note: "The same model without quantization. Four times the download and slower.",
    },
    RerankerSpec {
        id: "mxbai-rerank-base-q8",
        name: "mxbai Rerank Base v1 (8 bit)",
        repo: "mixedbread-ai/mxbai-rerank-base-v1",
        onnx: "onnx/model_quantized.onnx",
        params_millions: 184,
        download_mb: 244,
        context_tokens: 512,
        pair_tokens: 512,
        license: "Apache-2.0",
        note: "English only and a short context, long methods are judged by their first part.",
    },
];

pub fn find_reranker(id: &str) -> Option<&'static RerankerSpec> {
    RERANKERS.iter().find(|m| m.id == id)
}

/// Where the files of a model come from, the same for both kinds.
#[derive(Clone, Copy, Debug)]
pub struct Source {
    pub repo: &'static str,
    pub onnx: &'static str,
    pub onnx_extra: &'static [&'static str],
}

impl ModelSpec {
    pub fn source(&self) -> Source {
        Source { repo: self.repo, onnx: self.onnx, onnx_extra: self.onnx_extra }
    }
}

impl RerankerSpec {
    pub fn source(&self) -> Source {
        Source { repo: self.repo, onnx: self.onnx, onnx_extra: &[] }
    }
}

pub const TOKENIZER_FILES: &[&str] = &["tokenizer.json"];

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.id == id)
}
