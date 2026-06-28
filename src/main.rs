mod artifact;
#[cfg(feature = "build")]
mod build;
mod cli;
mod commands;
mod delta;
mod install;
mod installer;
mod layout;
mod parcel_manifest;
mod receipt;
mod repo;
mod resolver;
mod utils;

fn main() {
    if let Err(err) = commands::run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
