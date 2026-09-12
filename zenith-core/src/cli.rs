use clap::{Parser, ValueEnum};
use crate::log::LevelFilter;

/// Log level options for command-line argument.
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
    Off,
}

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => LevelFilter::Trace,
            LogLevel::Debug => LevelFilter::Debug,
            LogLevel::Info => LevelFilter::Info,
            LogLevel::Warn => LevelFilter::Warn,
            LogLevel::Error => LevelFilter::Error,
            LogLevel::Off => LevelFilter::Off,
        }
    }
}

/// Common command-line arguments for Zenith applications.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct EngineArgs {
    /// Set the log verbosity level
    #[arg(short = 'l', long = "log-level", value_enum, default_value_t = LogLevel::Info)]
    pub log_level: LogLevel,

    /// Additional positional arguments passed to the application
    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,
}

impl EngineArgs {
    /// Parse command-line arguments.
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
