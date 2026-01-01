use std::{error::Error, fmt::Display, sync::Arc};

use crate::core::entry::LogEntry;

#[derive(Debug)]
pub enum IteratorError {
    SourceError(Box<dyn Error + Send + Sync + 'static>),
}

impl Display for IteratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceError(err) => write!(f, "IteratorError::SourceError: {}", err),
        }
    }
}

impl Error for IteratorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceError(err) => Some(err.as_ref()),
        }
    }
}

pub trait SourceIterator {
    fn next(&mut self) -> Result<Option<Arc<LogEntry>>, IteratorError>;
    fn current(&self) -> Option<Arc<LogEntry>>;
}
