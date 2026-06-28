use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage configured package repositories.
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Download and cache repository indexes.
    Update,
    /// Search cached repositories for packages.
    Search(SearchArgs),
    /// Show package details from cached repositories.
    Info(InfoArgs),
    /// Install a package by name or from a local .parcel file.
    Install(InstallArgs),
    /// Upgrade one package or every installed package.
    Upgrade(UpgradeArgs),
    /// List installed packages.
    List,
    /// Remove a package from the system.
    Remove(RemoveArgs),
    #[cfg(feature = "build")]
    /// Build a .parcel archive from a package build manifest.
    Build(BuildArgs),
    #[cfg(feature = "build")]
    /// Build a .delta.parcel overlay between two .parcel archives.
    BuildDelta(BuildDeltaArgs),
}

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// Add a repository.
    Add(RepoAddArgs),
    /// Remove a repository.
    Remove(RepoRemoveArgs),
    #[cfg(feature = "build")]
    /// Generate a repository index from built artifacts.
    Index(RepoIndexArgs),
}

#[derive(Debug, Args)]
pub struct RepoAddArgs {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Args)]
pub struct RepoRemoveArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    pub query: String,
}

#[derive(Debug, Args)]
pub struct InfoArgs {
    pub package: String,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Package name or path to a .parcel archive file.
    pub package: String,
    /// Exact version to install when resolving from repositories.
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// Upgrade only the named package. Omit to upgrade everything installed.
    pub package: Option<String>,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    pub package: String,
}

#[cfg(feature = "build")]
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Path to a package directory or build manifest YAML file.
    pub manifest: String,
    /// Package release number appended to the manifest version.
    #[arg(short, long)]
    pub release: Option<u64>,
    /// Directory where Parcel creates its temporary build workspace.
    #[arg(long, default_value = ".parcel/build")]
    pub build_dir: String,
    /// Delete the build directory before building.
    #[arg(long)]
    pub clear: bool,
}

#[cfg(feature = "build")]
#[derive(Debug, Args)]
pub struct BuildDeltaArgs {
    #[arg(long)]
    pub from: String,
    #[arg(long)]
    pub to: String,
    #[arg(long)]
    pub output_dir: Option<String>,
}

#[cfg(feature = "build")]
#[derive(Debug, Args)]
pub struct RepoIndexArgs {
    pub artifacts_dir: String,
    #[arg(long)]
    pub base_url: String,
    #[arg(long)]
    pub output: Option<String>,
}
