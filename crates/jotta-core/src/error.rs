use std::fmt;

/// Errors that can occur in jotta-core.
#[derive(Debug)]
pub enum Error {
    /// jotta-cli exited with a non-zero exit code.
    /// `argv` is the full argument vector (excluding the binary) for debugging.
    CommandFailed { program: String, argv: Vec<String>, code: i32, stderr: String },
    /// A command could not be spawned (missing binary, etc.).
    Spawn(String, std::io::Error),
    /// JSON could not be parsed or did not have the expected shape.
    Parse(String),
    /// The daemon is not running / socket missing.
    DaemonUnavailable(String),
    /// A timeout elapsed while waiting for the daemon or a command.
    Timeout(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::CommandFailed { program, argv, code, stderr } => write!(
                f,
                "`{program} {}` exited with code {code}: {}",
                argv.join(" "),
                stderr.trim()
            ),
            Error::Spawn(program, io) => write!(f, "could not spawn `{program}`: {io}"),
            Error::Parse(msg) => write!(f, "could not parse output: {msg}"),
            Error::DaemonUnavailable(msg) => write!(f, "jottad unavailable: {msg}"),
            Error::Timeout(msg) => write!(f, "timed out: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
