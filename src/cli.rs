//! Command-line interface definitions.
//!
//! The CLI mirrors the package-manager operations users expect: install,
//! remove, list, search, update metadata, inspect package details, upgrade, and
//! manage remote repositories.

use clap::{Args, Parser, Subcommand};

/// Top-level command parser.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// User-facing package-manager commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install a local .parcel archive or a package from configured remotes.
    Install(InstallArgs),
    /// Build a .parcel archive from a package build manifest.
    Build(BuildArgs),
    /// Remove an installed package by name.
    Remove {
        /// Installed package name.
        name: String,
    },
    /// List installed packages.
    List,
    /// Show installed and repository metadata for a package.
    Info {
        /// Package name.
        name: String,
    },
    /// Search cached remote indexes.
    Search {
        /// Case-insensitive package name, homepage, or description fragment.
        query: String,
    },
    /// Download and cache package indexes from configured remotes.
    Update,
    /// Upgrade one installed package, or all installed packages.
    Upgrade(UpgradeArgs),
    /// Manage remote repositories.
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Path to a .parcel file or package name from a remote index.
    pub package: String,
}

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Path to a package directory or build manifest YAML file.
    pub manifest: String,
    /// Package release number appended to the manifest version.
    #[arg(short, long, default_value_t = 1)]
    pub release: u64,
    /// Target architecture. Defaults to the current machine architecture.
    #[arg(long)]
    pub arch: Option<String>,
    /// Directory where Parcel creates its temporary build workspace.
    #[arg(long)]
    pub build_dir: Option<String>,
    /// Directory where built .parcel archives are written.
    #[arg(short, long, default_value = "dist")]
    pub output_dir: String,
}

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// Optional installed package name. If omitted, all installed packages are checked.
    pub name: Option<String>,
    /// Run without asking for confirmation.
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Add a remote repository.
    Add {
        /// Local name used to identify the remote.
        name: String,
        /// Repository URL or direct parcel-index.db URL.
        url: String,
    },
    /// Remove a remote repository and its cached index.
    Remove {
        /// Remote name.
        name: String,
    },
    /// List configured remotes.
    List,
}
