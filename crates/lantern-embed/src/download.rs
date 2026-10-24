use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hf_hub::api::{Progress, sync::ApiBuilder};

use crate::models::{Source, TOKENIZER_FILES};

pub struct ModelFiles {
    pub onnx: PathBuf,
    pub tokenizer: PathBuf,
}

#[derive(Clone, Copy, Debug)]
pub struct DownloadProgress<'a> {
    pub file: &'a str,
    pub file_index: usize,
    pub file_count: usize,
    pub downloaded: usize,
    pub total: usize,
}

pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

fn wanted_files(spec: Source) -> Vec<&'static str> {
    let mut files = vec![spec.onnx];
    files.extend_from_slice(spec.onnx_extra);
    files.extend_from_slice(TOKENIZER_FILES);
    files
}

/// Whether every file of the model is already in the local cache.
pub fn is_downloaded(spec: Source, data_dir: &Path) -> bool {
    let cache = hf_hub::Cache::new(models_dir(data_dir));
    let repo = cache.model(spec.repo.to_owned());
    wanted_files(spec).iter().all(|file| repo.get(file).is_some())
}

struct Reporter<'a, F: FnMut(DownloadProgress)> {
    callback: &'a mut F,
    file: String,
    file_index: usize,
    file_count: usize,
    downloaded: usize,
    total: usize,
}

impl<F: FnMut(DownloadProgress)> Reporter<'_, F> {
    fn report(&mut self) {
        (self.callback)(DownloadProgress {
            file: &self.file,
            file_index: self.file_index,
            file_count: self.file_count,
            downloaded: self.downloaded,
            total: self.total,
        });
    }
}

impl<F: FnMut(DownloadProgress)> Progress for Reporter<'_, F> {
    fn init(&mut self, size: usize, filename: &str) {
        self.total = size;
        self.downloaded = 0;
        self.file = filename.to_owned();
        self.report();
    }

    fn update(&mut self, size: usize) {
        self.downloaded += size;
        self.report();
    }

    fn finish(&mut self) {
        self.downloaded = self.total;
        self.report();
    }
}

/// Fetches whatever is missing and returns where the files are. Files that are already
/// cached are not touched, so this is also how a downloaded model gets located.
pub fn fetch(spec: Source, data_dir: &Path, mut on_progress: impl FnMut(DownloadProgress)) -> Result<ModelFiles> {
    let dir = models_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    // Every model here is public. A token is not needed, and a stale one in the Hugging Face
    // cache gets even public files refused with a 401.
    let api = ApiBuilder::new().with_cache_dir(dir).with_token(None).with_progress(false).build()?;
    let repo = api.model(spec.repo.to_owned());
    let cache = hf_hub::Cache::new(models_dir(data_dir)).model(spec.repo.to_owned());

    let files = wanted_files(spec);
    let mut paths = Vec::with_capacity(files.len());
    for (file_index, file) in files.iter().enumerate() {
        let path = match cache.get(file) {
            Some(path) => path,
            None => {
                let reporter = Reporter { callback: &mut on_progress, file: (*file).to_owned(), file_index, file_count: files.len(), downloaded: 0, total: 0 };
                repo.download_with_progress(file, reporter).with_context(|| format!("downloading {file} from {}", spec.repo))?
            }
        };
        paths.push(path);
    }

    // External weights are looked up next to the graph by onnxruntime, make sure they are there.
    let onnx = paths[0].clone();
    for (file, path) in files.iter().zip(&paths).skip(1).take(spec.onnx_extra.len()) {
        anyhow::ensure!(path.parent() == onnx.parent(), "{file} ended up in a different folder than {}, the model can not be loaded", spec.onnx);
    }
    let tokenizer = paths[1 + spec.onnx_extra.len()].clone();
    Ok(ModelFiles { onnx, tokenizer })
}
