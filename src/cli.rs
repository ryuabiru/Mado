use std::path::PathBuf;

use clap::Parser;

/// A minimal GUI client that uses Neovim as its editing engine.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Files to open in Neovim.
    #[arg(value_name = "FILE")]
    pub files: Vec<PathBuf>,
}
