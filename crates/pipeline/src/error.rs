use std::path::PathBuf;

/// Every failure the pipeline can report. Messages are written for the person
/// using the app, not for a developer: they say what happened and what to try.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Couldn't read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },

    #[error("{path} is empty (0 bytes). The file may not have finished copying.")]
    EmptyFile { path: PathBuf },

    #[error("This file isn't a photo the editor can open, or it is damaged. ({detail})")]
    Unreadable { detail: String },

    #[error("This photo type isn't supported yet: {detail}")]
    Unsupported { detail: String },

    #[error("Couldn't write {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },

    #[error("JPEG encoding failed: {0}")]
    Encode(String),

    #[error("This edit uses a look that is missing: {0}")]
    Look(String),

    #[error("The saved edit file {path} can't be used: {detail}")]
    Sidecar { path: PathBuf, detail: String },
}

pub type Result<T> = std::result::Result<T, PipelineError>;
