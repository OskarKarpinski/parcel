use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Build a .parcel archive from a package build manifest.
    Build(BuildArgs),
}

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Path to a package directory or build manifest YAML file.
    pub manifest: String,
    /// Package release number appended to the manifest version.
    #[arg(short, long, default_value_t = 1)]
    pub release: u64,
    /// Directory where Parcel creates its temporary build workspace.
    #[arg(long, default_value = ".parcel/build")]
    pub build_dir: String,
    /// Delete the build directory before building.
    #[arg(long)]
    pub clear: bool,
}
