//! Cross encoder reranking. An embedding compares a question with a text it has never seen
//! next to it, a reranker reads both together and says how well the text answers the question.
//! Slower, so it only looks at the candidates the search already found.

use std::borrow::Cow;

use anyhow::{Result, anyhow, ensure};
use ort::{
    session::{Session, SessionInputValue, builder::GraphOptimizationLevel},
    value::Tensor,
};
use tokenizers::{Tokenizer, TruncationDirection, TruncationParams, TruncationStrategy};

use crate::{download::ModelFiles, models::RerankerSpec};

const BATCH: usize = 8;

pub struct Reranker {
    pub spec: &'static RerankerSpec,
    session: Session,
    tokenizer: Tokenizer,
    input_names: Vec<String>,
    output_name: String,
    pad_id: i64,
}

impl Reranker {
    pub fn load(spec: &'static RerankerSpec, files: &ModelFiles, threads: usize) -> Result<Self> {
        let mut tokenizer = Tokenizer::from_file(&files.tokenizer).map_err(|e| anyhow!("loading {}: {e}", files.tokenizer.display()))?;
        // Only the document side gets cut. The start of a method carries its signature and
        // most of what it is about, and the question always stays whole.
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: spec.pair_tokens,
                strategy: TruncationStrategy::OnlySecond,
                stride: 0,
                direction: TruncationDirection::Right,
            }))
            .map_err(|e| anyhow!("{e}"))?;
        tokenizer.with_padding(None);
        let pad_id = ["<pad>", "[PAD]"].iter().find_map(|t| tokenizer.token_to_id(t)).map_or(0, i64::from);

        let session = Session::builder()
            .map_err(|e| anyhow!("{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("{e}"))?
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(&files.onnx)
            .map_err(|e| anyhow!("loading {}: {e}", files.onnx.display()))?;

        let input_names: Vec<String> = session.inputs().iter().map(|i| i.name().to_owned()).collect();
        for name in &input_names {
            ensure!(
                matches!(name.as_str(), "input_ids" | "attention_mask" | "token_type_ids"),
                "{} wants an input called `{name}`, which the reranker does not know how to fill",
                spec.id
            );
        }
        let outputs: Vec<String> = session.outputs().iter().map(|o| o.name().to_owned()).collect();
        let output_name = outputs.iter().find(|o| o.as_str() == "logits").or(outputs.first()).cloned().ok_or_else(|| anyhow!("{} has no outputs", spec.id))?;

        Ok(Self { spec, session, tokenizer, input_names, output_name, pad_id })
    }

    /// How well each document answers the question, 0 to 1, in the order they were given.
    pub fn score(&mut self, question: &str, documents: &[String]) -> Result<Vec<f32>> {
        let pairs: Vec<(String, String)> = documents.iter().map(|d| (question.to_owned(), d.clone())).collect();
        let encodings = self.tokenizer.encode_batch(pairs, true).map_err(|e| anyhow!("{e}"))?;

        // Similar lengths together, a batch is as slow as its longest pair.
        let mut order: Vec<usize> = (0..encodings.len()).collect();
        order.sort_by_key(|i| encodings[*i].len());

        let mut scores = vec![0f32; encodings.len()];
        for group in order.chunks(BATCH) {
            let longest = group.iter().map(|i| encodings[*i].len()).max().unwrap_or(0);
            let mut ids = vec![self.pad_id; group.len() * longest];
            let mut mask = vec![0i64; group.len() * longest];
            let mut types = vec![0i64; group.len() * longest];
            for (row, index) in group.iter().enumerate() {
                let encoding = &encodings[*index];
                for col in 0..encoding.len() {
                    ids[row * longest + col] = i64::from(encoding.get_ids()[col]);
                    types[row * longest + col] = i64::from(encoding.get_type_ids()[col]);
                    mask[row * longest + col] = 1;
                }
            }

            let shape = [group.len(), longest];
            let mut inputs: Vec<(Cow<str>, SessionInputValue)> = Vec::with_capacity(self.input_names.len());
            for name in &self.input_names {
                let data = match name.as_str() {
                    "input_ids" => ids.clone(),
                    "attention_mask" => mask.clone(),
                    _ => types.clone(),
                };
                inputs.push((Cow::Owned(name.clone()), Tensor::from_array((shape, data))?.into()));
            }

            let outputs = self.session.run(inputs)?;
            let (out_shape, data) = outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
            let dims: Vec<usize> = out_shape.iter().map(|d| *d as usize).collect();
            let per_row = match dims.as_slice() {
                [b] if *b == group.len() => 1,
                [b, n] if *b == group.len() && *n >= 1 => *n,
                shape => anyhow::bail!("{}: output `{}` has shape {shape:?}, expected one score per pair", self.spec.id, self.output_name),
            };
            for (row, index) in group.iter().enumerate() {
                // With two classes the last one is "relevant".
                let logit = data[row * per_row + per_row - 1];
                scores[*index] = 1.0 / (1.0 + (-logit).exp());
            }
        }
        Ok(scores)
    }
}
