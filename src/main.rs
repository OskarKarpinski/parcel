#[cfg(feature = "build")]
mod build;
mod cli;
mod commands;
mod install;
mod parcel_manifest;
mod utils;

fn main() {
    if let Err(err) = commands::run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
