//! The error type of `runtime` (feature `runtime`): what went wrong reading the paks or their data, or writing the
//! output. The rest of the library reports `anyhow` errors; they arrive here as `Format` (or `Io` when a read failed).

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A pak could not be read.
    Io(String),
    /// Data that is not what the converter expects (not a zip, a bad map, …).
    Format(String),
    /// Readable paks, but no `maps/*.bsp` in them (not Quake III Arena's `baseq3`).
    NoMaps,
    /// The output sink refused a file.
    Sink(String),
    /// The caller's progress callback asked to stop.
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "read error: {e}"),
            Error::Format(e) => write!(f, "{e}"),
            Error::NoMaps => write!(f, "no maps in these .pk3 files (not Quake III Arena's baseq3 folder?)"),
            Error::Sink(e) => write!(f, "could not write: {e}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Error {
        let io = e.chain().any(|c| {
            c.is::<std::io::Error>()
                || matches!(c.downcast_ref::<zip::result::ZipError>(), Some(zip::result::ZipError::Io(_)))
        });
        if io { Error::Io(format!("{e:#}")) } else { Error::Format(format!("{e:#}")) }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Format(format!("JSON: {e}"))
    }
}
