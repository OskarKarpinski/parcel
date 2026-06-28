use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command, RepoCommand};

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Repo { command } => match command {
            RepoCommand::Add(args) => crate::repo::add_repo(&args),
            RepoCommand::Remove(args) => crate::repo::remove_repo(&args),
            #[cfg(feature = "build")]
            RepoCommand::Index(args) => crate::repo::build_repo_index(&args),
        },
        Command::Update => crate::repo::update_indexes(),
        Command::Search(args) => crate::repo::search_packages(&args),
        Command::Info(args) => crate::repo::show_package_info(&args),
        Command::Install(args) => crate::install::install_package(&args),
        Command::Upgrade(args) => crate::install::upgrade_packages(&args),
        Command::List => crate::receipt::list_installed_packages(),
        Command::Remove(args) => crate::install::remove_package(&args),
        #[cfg(feature = "build")]
        Command::Build(args) => crate::build::build_package(&args),
        #[cfg(feature = "build")]
        Command::BuildDelta(args) => crate::delta::build_delta_command(&args),
    }
}
