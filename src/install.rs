use std::path::Path;

use anyhow::Result;

use crate::cli::InstallArgs;

pub fn install_package(args: &InstallArgs) -> Result<()> {
    // Check if file exists
    let package_path = Path::new(&args.package);
    if package_path.exists() {
        install_from_file(package_path)?;
    } else {
        // TODO
        todo!("Implement repository")
    }

    Ok(())
}

fn install_from_file(package_path: &Path) -> Result<()> {
    println!("Installing package from file: {:?}", package_path);

    Ok(())
}
