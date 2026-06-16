//! Top-level command dispatch.

use anyhow::Result;
use clap::Parser;

use crate::build::build_package;
use crate::cli::{Cli, Command};
use crate::packages::{
    install_command, list_installed, remove_package, show_package_info, upgrade_packages,
};
use crate::paths::Paths;
use crate::repositories::{remote_command, search_indexes, update_indexes};

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover()?;

    match cli.command {
        Command::Install(args) => install_command(&paths, &args.package),
        Command::Build(args) => build_package(&args),
        Command::Remove { name } => remove_package(&paths, &name),
        Command::List => list_installed(&paths),
        Command::Info { name } => show_package_info(&paths, &name),
        Command::Search { query } => search_indexes(&paths, &query),
        Command::Update => update_indexes(&paths),
        Command::Upgrade(args) => upgrade_packages(&paths, args.name.as_deref(), args.yes),
        Command::Remote { command } => remote_command(&paths, command),
    }
}
