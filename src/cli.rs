use std::path::PathBuf;

use clap::Parser;

/// A minimal GUI client that uses Neovim as its editing engine.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Read Mado settings from this file instead of the platform default.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Files to open in Neovim.
    #[arg(value_name = "FILE")]
    pub files: Vec<PathBuf>,
}
