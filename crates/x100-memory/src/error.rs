use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("git {args}: {stderr}")]
    Git { args: String, stderr: String },
    #[error("{0}")]
    Invalid(String),
    #[error("there is no {0}")]
    NotFound(String),
    #[error("the branch {branch} does not merge into main: {why}")]
    Conflict { branch: String, why: String },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}
