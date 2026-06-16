//! Parcel is a user-space package manager for Linux desktop applications and
//! developer tools.
//!
//! The binary is intentionally thin. Command parsing, repository handling,
//! package archive extraction, and database persistence live in separate
//! modules so each part can evolve independently as Parcel grows toward a
//! DNF-like workflow without requiring root privileges.

mod actions;
mod archive;
mod build;
mod cli;
mod commands;
mod models;
mod packages;
mod paths;
mod repositories;
mod storage;
mod version;

fn main() {
    if let Err(err) = commands::run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
