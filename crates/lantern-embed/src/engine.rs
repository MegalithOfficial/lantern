use std::borrow::Cow;

use anyhow::{Result, anyhow, bail, ensure};
use ort::{
    session::{Session, SessionInputValue, builder::GraphOptimizationLevel},
    value::Tensor,
};
use tokenizers::Tokenizer;

use crate::{
    download::ModelFiles,
    models::{ModelSpec, Pooling},
};

pub struct Engine {
    pub spec: &'static ModelSpec,
    session: Session,
    tokenizer: Tokenizer,
    input_names: Vec<String>,
    output_name: String,
    pad_id: i64,
}

impl Engine {
    pub fn load(spec: &'static ModelSpec, files: &ModelFiles, threads: usize) -> Result<Self> {
        let mut tokenizer = Tokenizer::from_file(&files.tokenizer).map_err(|e| anyhow!("loading {}: {e}", files.tokenizer.display()))?;
        // Chunking keeps every input inside the context. Truncation stays off so that a bug
        // there shows up as an error instead of as silently shortened text.
        tokenizer.with_truncation(None).map_err(|e| anyhow!("{e}"))?;
        tokenizer.with_padding(None);
        let pad_id = tokenizer
            .get_padding()
            .map(|p| i64::from(p.pad_id))
            .or_else(|| ["<pad>", "[PAD]", "<|endoftext|>"].iter().find_map(|t| tokenizer.token_to_id(t)).map(i64::from))
            .unwrap_or(0);

        // The builder errors hold raw pointers and can not cross into anyhow as they are.
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
                matches!(name.as_str(), "input_ids" | "attention_mask" | "token_type_ids" | "position_ids"),
                "{} wants an input called `{name}`, which this engine does not know how to fill",
                spec.id
            );
        }

        let outputs: Vec<String> = session.outputs().iter().map(|o| o.name().to_owned()).collect();
        let output_name = match spec.pooling {
            Pooling::Output(name) => {
                ensure!(outputs.iter().any(|o| o == name), "{} has no output `{name}`, only {outputs:?}", spec.id);
                name.to_owned()
            }
            _ => outputs
                .iter()
                .find(|o| o.as_str() == "last_hidden_state" || o.as_str() == "token_embeddings")
                .or(outputs.first())
                .cloned()
                .ok_or_else(|| anyhow!("{} has no outputs", spec.id))?,
        };

        Ok(Self { spec, session, tokenizer, input_names, output_name, pad_id })
    }

    /// Tokens the model sees for `text`, special tokens included.
    pub fn count_tokens(&self, text: &str) -> Result<usize> {
        Ok(self.tokenizer.encode(text, true).map_err(|e| anyhow!("{e}"))?.len())
    }

    pub fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        let text = format!("{}{query}", self.spec.query_prefix);
        let mut vectors = self.embed(&[text])?;
        Ok(vectors.remove(0))
    }

    pub fn document_text(&self, title: &str, body: &str) -> String {
        format!("{}{body}", self.spec.document_prefix.replace("{title}", title))
    }

    /// Embeds already prefixed texts. Returns unit length vectors cut to `stored_dims`.
    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self.tokenizer.encode_batch(texts.iter().map(String::as_str).collect(), true).map_err(|e| anyhow!("{e}"))?;
        let longest = encodings.iter().map(|e| e.len()).max().unwrap_or(0);
        if longest > self.spec.context_tokens {
            bail!("an input has {longest} tokens, {} takes {} at most", self.spec.id, self.spec.context_tokens);
        }

        let batch = encodings.len();
        let mut ids = vec![self.pad_id; batch * longest];
        let mut mask = vec![0i64; batch * longest];
        let mut positions = self.input_names.iter().any(|name| name == "position_ids").then(|| vec![0i64; batch * longest]);
        let mut lengths = Vec::with_capacity(batch);
        for (row, encoding) in encodings.iter().enumerate() {
            let len = encoding.len();
            lengths.push(len);
            for (col, id) in encoding.get_ids().iter().enumerate() {
                ids[row * longest + col] = i64::from(*id);
                mask[row * longest + col] = 1;
                if let Some(positions) = &mut positions {
                    positions[row * longest + col] = col as i64;
                }
            }
        }

        let shape = [batch, longest];
        let mut inputs: Vec<(Cow<str>, SessionInputValue)> = Vec::with_capacity(self.input_names.len());
        for name in &self.input_names {
            let data = match name.as_str() {
                "input_ids" => std::mem::take(&mut ids),
                "attention_mask" => std::mem::take(&mut mask),
                "position_ids" => positions.take().unwrap_or_default(),
                _ => vec![0i64; batch * longest],
            };
            inputs.push((Cow::Owned(name.clone()), Tensor::from_array((shape, data))?.into()));
        }

        let outputs = self.session.run(inputs)?;
        let (out_shape, data) = outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
        let dims: Vec<usize> = out_shape.iter().map(|d| *d as usize).collect();

        let mut vectors = Vec::with_capacity(batch);
        match (self.spec.pooling, dims.as_slice()) {
            (Pooling::Output(_), [b, hidden]) if *b == batch => {
                for row in 0..batch {
                    vectors.push(data[row * hidden..(row + 1) * hidden].to_vec());
                }
            }
            (Pooling::Mean | Pooling::Cls | Pooling::LastToken, [b, steps, hidden]) if *b == batch && *steps == longest => {
                for (row, len) in lengths.iter().copied().enumerate() {
                    let token = |t: usize| &data[(row * steps + t) * hidden..(row * steps + t + 1) * hidden];
                    let vector = match self.spec.pooling {
                        Pooling::Cls => token(0).to_vec(),
                        // Padding is on the right, so the last real token is at len - 1.
                        Pooling::LastToken => token(len.saturating_sub(1)).to_vec(),
                        _ => {
                            let mut sum = vec![0f32; *hidden];
                            for t in 0..len {
                                for (s, v) in sum.iter_mut().zip(token(t)) {
                                    *s += v;
                                }
                            }
                            let n = len.max(1) as f32;
                            sum.iter_mut().for_each(|s| *s /= n);
                            sum
                        }
                    };
                    vectors.push(vector);
                }
            }
            (pooling, shape) => bail!("{}: output `{}` has shape {shape:?}, which does not fit {pooling:?}", self.spec.id, self.output_name),
        }

        for vector in &mut vectors {
            ensure!(vector.len() >= self.spec.stored_dims, "{} produced {} dims, expected {}", self.spec.id, vector.len(), self.spec.dims);
            vector.truncate(self.spec.stored_dims);
            normalize(vector)?;
        }
        Ok(vectors)
    }
}

fn normalize(vector: &mut [f32]) -> Result<()> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    ensure!(norm.is_finite() && norm > 0.0, "the model returned a vector that can not be normalized");
    vector.iter_mut().for_each(|v| *v /= norm);
    Ok(())
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
