use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Install(args) => crate::install::install_package(&args),
        Command::Remove => todo!("Implement package removal"),
        #[cfg(feature = "build")]
        Command::Build(args) => crate::build::build_package(&args),
    }
}
