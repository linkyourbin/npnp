use thiserror::Error;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid response from LCEDA: {0}")]
    InvalidResponse(String),

    #[error("no results found for keyword: {0}")]
    NoResults(String),

    #[error("invalid index {index}. Valid range: 1..{max} for keyword '{keyword}'.")]
    InvalidIndex {
        keyword: String,
        index: usize,
        max: usize,
    },

    #[error("selected component has no 3D model UUID")]
    MissingModelUuid,

    #[error("selected component has no symbol/footprint uuid")]
    MissingSymbolOrFootprint,

    #[error("{0}")]
    Other(String),
}
