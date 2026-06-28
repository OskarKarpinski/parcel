# Parcel

Parcel is a Linux user-space package manager for desktop applications and
developer tools, with a Homebrew-like cellar layout and repository-driven
binary installs.

The detailed package format specification lives in [Parcel.md](Parcel.md).

## Status

Parcel currently supports:

- building `.parcel` archives from YAML package manifests
- building `.delta.parcel` overlay updates between package versions
- generating static `parcel-index.db` repository indexes
- adding repositories, caching indexes, searching packages, showing info
- installing, upgrading, listing, and removing user-space packages

## Install From Source

Build the binary with Cargo:

```bash
cargo build --release
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
parcel repo add <name> <url>
parcel update
parcel search <query>
parcel info <name>
parcel install <name> [--version <ver>]
parcel install </path/to/package.parcel>
parcel upgrade [<name>]
parcel list
parcel remove <name>
parcel build [options] <manifest-or-package-dir>
parcel build-delta --from <old.parcel> --to <new.parcel>
parcel repo index <artifacts-dir> --base-url <base-url-or-path>
```

Installed packages live under XDG directories:

- `$XDG_DATA_HOME/parcel/cellar/<name>/<version>/`
- `$XDG_DATA_HOME/parcel/opt/<name>`
- `$XDG_STATE_HOME/parcel/receipts/<name>.json`
- `$XDG_CACHE_HOME/parcel/indexes/`
- `$XDG_CONFIG_HOME/parcel/repos.d/`

## Build A Package

Package manifests live naturally under `packages/<name>/`. The example package
can be built with:

```bash
cargo run -- build packages/example
```

By default, Parcel writes the built archive into the build workspace. The
generated archive name follows:

```text
<name>-<version>-<release>-<arch>.parcel
```

Example:

```text
.parcel/build/example/example-1.0.0-1-x86_64.parcel
```

Useful build options:

```bash
cargo run -- build packages/example --release 2
cargo run -- build packages/example --build-dir /tmp/parcel-build
```

`--build-dir` selects where Parcel creates its temporary build workspace. Parcel
stores sources, build artifacts, payload output, and the final `.parcel`
archive there.

## Build A Delta

Generate an overlay delta between two built package archives:

```bash
cargo run -- build-delta \
  --from .parcel/build/example/example-1.0.0-1-x86_64.parcel \
  --to .parcel/build/example/example-1.1.0-1-x86_64.parcel
```

The output name follows:

```text
<name>-<from-version>-<to-version>-<arch>.delta.parcel
```

## Generate A Repository Index

Create a static repository index from a directory containing `.parcel` and
`.delta.parcel` artifacts:

```bash
cargo run -- repo index .parcel/build/example --base-url https://example.com/releases
```

For local development you can also use a filesystem path as `--base-url`.

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


## Package Archive Format

A `.parcel` file is an uncompressed outer tar archive containing:

```text
manifest.yml
data.tar.zst
```

A `.delta.parcel` file contains:

```text
delta.yml
manifest.yml
delta-data.tar.zst
```

Install actions in `manifest.yml` describe which payload files should be linked
or copied by consumers that install `.parcel` archives.

## Development Checks

Run the standard checks before submitting changes:

```bash
cargo fmt --check
cargo check
cargo test
```
