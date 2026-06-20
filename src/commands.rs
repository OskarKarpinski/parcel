//! Top-level command dispatch.

use anyhow::Result;
use clap::Parser;

use crate::build::build_package;
use crate::cli::{Cli, Command};

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Build(args) => build_package(&args),
    }
}
