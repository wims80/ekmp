pub(crate) mod auth;
pub(crate) mod backend;
pub(crate) mod esi;
pub(crate) mod http;
#[cfg(feature = "gui")]
pub(crate) mod images;
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) mod simulation;
pub(crate) mod zkill;

use std::fmt;

/// Failure from an external API or credential operation.
///
/// Only `Other` is recoverable: callers may degrade or record a per-character
/// failure, while every other kind must abort the surrounding operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApiError {
    Cancelled,
    RateLimited(String),
    Persistence(String),
    Other(String),
}

impl ApiError {
    pub(crate) fn is_fatal(&self) -> bool {
        !matches!(self, Self::Other(_))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("Operation cancelled"),
            Self::RateLimited(message) | Self::Persistence(message) | Self::Other(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for ApiError {}

impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<&str> for ApiError {
    fn from(message: &str) -> Self {
        Self::Other(message.into())
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;
