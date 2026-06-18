# Parcel

Parcel is a Linux user-space package manager for desktop applications and
developer tools. It installs packages under the current user's home directory,
without requiring root access or writing into system package-manager locations.

The detailed package format specification lives in [Parcel.md](Parcel.md).

## Status

Parcel currently supports:

- Installing local `.parcel` archives
- Installing packages from cached remote indexes
- Removing installed packages cleanly
- Listing and inspecting installed packages
- Searching cached repository indexes
- Updating cached repository indexes
- Upgrading installed packages from remotes
- Managing remote repositories

When compiled with the `build` feature, Parcel also supports building `.parcel`
archives from YAML package manifests.

Delta packages are reserved in the format but are not implemented yet.

## Install From Source

Build the binary with Cargo:

```bash
cargo build --release
```

Build with package-authoring support enabled:

```bash
cargo build --release --features build
```

Run it from the build output:

```bash
target/release/parcel --help
```

During development, use:

```bash
cargo run -- --help
```

## Command Overview

```bash
parcel install <package>
parcel remove <name>
parcel list
parcel info <name>
parcel search <query>
parcel update [name] [--yes]
parcel remote add <name> <url>
parcel remote remove <name>
parcel remote list
```

When compiled with `--features build`, the CLI also includes:

```bash
parcel build [options] <manifest-or-package-dir>
```

`<package>` can be either a path to a local `.parcel` archive or a package name
from a cached remote repository index.

## User-Space Layout

Parcel stores all installed package state under the current user's home
directory:

```text
$HOME/.local/share/parcel/apps/<name>/<version>/
$HOME/.local/share/parcel/parcel.db
$HOME/.local/share/parcel/remotes.json
$HOME/.local/share/parcel/indexes/
```

Package actions can expose files into standard XDG user locations:

```text
$HOME/.local/bin/
$HOME/.local/share/applications/
$HOME/.local/share/icons/
$HOME/.local/share/man/
```

## Build A Package

Package manifests live naturally under `packages/<name>/`. The example package
can be built when Parcel is compiled with the `build` feature:

```bash
cargo run --features build -- build packages/example
```

By default, Parcel writes the built archive into the same directory as the
package manifest. The generated archive name follows:

```text
<name>-<version>-<release>-<arch>.parcel
```

Example:

```text
packages/example/example-1.0.0-1-x86_64.parcel
```

Useful build options:

```bash
cargo run --features build -- build packages/example --release 2
cargo run --features build -- build packages/example --arch x86_64
cargo run --features build -- build packages/example --output-dir /tmp/parcel-dist
cargo run --features build -- build packages/example --build-dir /tmp/parcel-build
```

`--build-dir` selects where Parcel creates its temporary build workspace. Parcel
creates a unique `parcel-build-*` directory inside that location and removes it
after the build completes.

## Build Manifest

A package build manifest is a YAML file such as
[packages/example/example.yml](packages/example/example.yml):

```yaml
name: example
version: 1.0.0
description: Example parcel package
architecture:
  - x86_64
  - aarch64

delta: false
compression: zstd

source:
  - ./example.desktop

source-x86-64:
  - https://example.com/bin-x86_64.zip:abc123...

source-aarch64:
  - https://example.com/bin-aarch64.zip:def456...

build_script: |
  echo "Building..."
  echo "#!/bin/bash" > example.sh
  echo "echo \"Hello World\"" >> example.sh
  chmod +x example.sh

install_script: |
  mkdir -p $OUTPUT_DIR/bin
  mv ./example.sh $OUTPUT_DIR/bin/example
  mv $SOURCE_DIR/example.desktop $OUTPUT_DIR/example.desktop

files:
  bin:
    - bin/example:link
  desktop:
    - example.desktop:copy
```

Build scripts run in an isolated build directory with these environment
variables:

- `SOURCE_DIR`: directory containing resolved local or downloaded source files
- `OUTPUT_DIR`: directory that becomes the package payload

The `files` section declares which payload files should be linked or copied into
XDG locations during install. Entries use `path:link` or `path:copy`.

## Source Checksums

Sources can include an optional BLAKE2b checksum suffix:

```yaml
source:
  - ./local-file.txt
  - ./checked-file.txt:abc123...
  - https://example.com/archive.tar.gz:def456...
```

If no checksum suffix is present, Parcel skips source verification for that
source.

Architecture-specific sources can be declared next to shared sources:

```yaml
source:
  - ./common.desktop

source-x86-64:
  - https://example.com/bin.zip:abc123...

source-aarch64:
  - https://example.com/bin-aarch64.zip:def456...
```

`source` entries are always resolved. `source-x86-64` is resolved only when
building for `x86_64`, and `source-aarch64` is resolved only when building for
`aarch64`. `source-x86_64` is also accepted as an alias.

## Install And Remove

Install a local package:

```bash
cargo run -- install packages/example/example-1.0.0-1-x86_64.parcel
```

List installed packages:

```bash
cargo run -- list
```

Inspect package metadata:

```bash
cargo run -- info example
```

Remove the package:

```bash
cargo run -- remove example
```

## Remote Repositories

Remote repositories expose a zstd-compressed JSON index named
`parcel-index.db`. For GitHub repositories, Parcel expands:

```bash
parcel remote add default https://github.com/<owner>/<repo>
```

to:

```text
https://github.com/<owner>/<repo>/releases/download/parcel-index/parcel-index.db
```

Update cached indexes and installed packages:

```bash
parcel update
```

Search cached indexes:

```bash
parcel search example
```

Install a package from cached remotes:

```bash
parcel install example
```

Update one installed package:

```bash
parcel update example
```

Add `--yes` to apply available package updates without prompting.

## Package Archive Format

A `.parcel` file is an uncompressed outer tar archive containing:

```text
manifest.yml
data.tar.zst
```

`data.tar.xz` is also supported when the build manifest uses
`compression: xz`.

The payload is extracted under:

```text
$HOME/.local/share/parcel/apps/<name>/<version>/
```

Install actions then symlink or copy selected payload files into user XDG
locations.

## Development Checks

Run the standard checks before submitting changes:

```bash
cargo fmt --check
cargo check
cargo test
```
