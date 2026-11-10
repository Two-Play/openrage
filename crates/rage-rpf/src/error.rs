use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RpfError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}: not an RPF7 archive (magic {1:#010x})")]
    BadMagic(String, u32),
    #[error("{0}: archive is encrypted but no keys are loaded (run `openrage keys` first)")]
    KeysRequired(String),
    #[error("{archive}: corrupt table of contents: {reason}")]
    CorruptToc { archive: String, reason: String },
    #[error("{archive}/{entry}: entry data lies outside the archive")]
    OutOfBounds { archive: String, entry: String },
    #[error("{archive}/{entry}: decompression failed: {source}")]
    Inflate { archive: String, entry: String, source: std::io::Error },
    #[error("{path}: invalid resource: {reason}")]
    BadResource { path: String, reason: String },
    #[error("{0} is not a directory")]
    NotADirectory(String),
    #[error("{0} is not a nested archive")]
    NotAnArchive(String),
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("not a file or directory: {0}")]
    BadPath(PathBuf),
}
