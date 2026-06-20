# Parcel Package Specification

This document describes the package, build-manifest, repository-index, and
user-state formats used by Parcel.

Parcel is a user-space Linux package manager. It installs application payloads
under the current user's home directory and exposes selected files into standard
XDG locations such as `$HOME/.local/bin` and
`$HOME/.local/share/applications`.

## Status

This specification documents the currently implemented format.

- Package archives with `.parcel` extension are supported.
- Payload compression supports `zstd` and `xz`.
- Build manifests are YAML files consumed by `parcel build` when Parcel is
  compiled with the `build` feature.
- Remote repository indexes are zstd-compressed JSON documents.
- Delta packages are reserved in the repository and build formats, but package
  generation and installation for deltas are not implemented.

## Naming

Built package archive names should use:

```text
<name>-<version>-<release>-<arch>.parcel
```

Example:

```text
example-1.0.0-1-x86_64.parcel
```

Inside package metadata, the package `version` includes the release suffix:

```text
<upstream-version>-<release>
```

Example:

```text
1.0.0-1
```

Version comparison prefers semantic versions with Parcel release suffixes. If a
version cannot be parsed as `<semver>-<release>`, Parcel falls back to string
comparison.

## Architectures

Parcel uses Rust's current target architecture name for the running machine.
Common values are:

- `x86_64`
- `aarch64`

Install rejects a package whose archive manifest `arch` does not match the
current machine architecture.

## Package Archive Format

A `.parcel` file is an uncompressed outer tar archive. The outer archive must
contain:

```text
manifest.yml
data.tar.zst
```

or:

```text
manifest.yml
data.tar.xz
```

`manifest.yml` is a YAML package manifest. The `data.tar.*` member is a
compressed tar archive containing the package payload.

Unknown members in the outer archive are ignored by the current reader. Package
authors should not rely on unknown members for installed behavior.

### Payload Archive

The payload archive is extracted into:

```text
$HOME/.local/share/parcel/apps/<name>/<version>/
```

Payload paths must be relative paths. Parcel rejects archive entries that are
empty, absolute, contain `..`, or otherwise escape the install root.

Payload files can then be exposed through install actions defined in
`manifest.yml`.

## Archive Manifest

`manifest.yml` is stored inside a `.parcel` archive.

Required fields:

- `name`: package name.
- `version`: full package version including release suffix.
- `arch`: target architecture.
- `description`: short package description.
- `homepage`: package homepage URL. Use an empty string if no homepage is known.

Optional fields:

- `actions`: list of install actions. Defaults to an empty list.

Example:

```yaml
name: example
version: 1.0.0-1
arch: x86_64
description: Example parcel package
homepage: https://example.com
actions:
  - source: bin/example
    target: bin
    type: link
  - source: example.desktop
    target: desktop
    type: copy
```

### Install Actions

An action exposes one payload file into a user-visible location.

Fields:

- `source`: relative path inside the extracted payload.
- `target`: target category.
- `type`: action type, either `link` or `copy`.

Supported target categories:

| Target | Destination |
| --- | --- |
| `bin` | `$HOME/.local/bin` |
| `applications` | `$HOME/.local/share/applications` |
| `desktop` | alias for `applications` |
| `icons` | `$HOME/.local/share/icons` |
| `man` | `$HOME/.local/share/man` |

For `bin` and `applications`, Parcel installs the source file basename into the
destination directory.

For `icons`, Parcel preserves the path below `share/icons` when the source path
starts with that prefix. Otherwise, it installs the source file basename.

For `man`, Parcel preserves the path below `share/man` when the source path
starts with that prefix. Otherwise, it installs the source file basename.

Parcel refuses to overwrite an existing destination path. If an action fails
during install, already applied actions for that package install are rolled back.

During removal, copied files are removed from their recorded destination. Linked
files are removed only when the destination is still a symlink pointing to the
installed package payload.

## Build Manifest Format

Build manifests are package-authoring YAML files used by:

```bash
parcel build <manifest-or-package-dir>
```

When a directory is passed, it must contain exactly one `.yml` or `.yaml` file.

Required fields:

- `name`: package name.
- `version`: upstream package version without the Parcel release suffix.
- `description`: short package description.
- `architecture`: non-empty list of supported architecture names.

Optional fields:

- `homepage`: package homepage URL.
- `delta`: boolean. Defaults to `false`. Reserved; delta generation is not
  implemented.
- `compression`: `zstd` or `xz`. Defaults to `zstd`.
- `source`: shared source entries resolved for every build.
- `source-x86-64`: source entries resolved only for `x86_64` builds.
- `source-x86_64`: alias for `source-x86-64`.
- `source-aarch64`: source entries resolved only for `aarch64` builds.
- `build_script`: shell script run before install.
- `install_script`: shell script that writes the package payload.
- `files`: map of install action target categories to `path:type` entries.

Example:

```yaml
name: example
version: 1.0.0
description: Example parcel package
homepage: https://example.com
architecture:
  - x86_64
  - aarch64

delta: false
compression: zstd

source:
  - ./example.desktop

source-x86-64:
  - https://example.com/bin-x86_64.zip:abc123

source-aarch64:
  - https://example.com/bin-aarch64.zip:def456

build_script: |
  echo '#!/bin/sh' > example
  echo 'echo hello from Parcel' >> example
  chmod +x example

install_script: |
  mkdir -p "$OUTPUT_DIR/bin"
  mv example "$OUTPUT_DIR/bin/example"
  cp "$SOURCE_DIR/example.desktop" "$OUTPUT_DIR/example.desktop"

files:
  bin:
    - bin/example:link
  desktop:
    - example.desktop:copy
```

### Build Scripts

`build_script` and `install_script` run with:

```text
bash --noprofile --norc -x -c <script>
```

Both scripts run in an isolated build directory. Parcel sets:

- `SOURCE_DIR`: directory containing resolved local and downloaded source files.
- `OUTPUT_DIR`: directory that becomes the package payload.

`install_script` must create at least one file in `OUTPUT_DIR`; otherwise the
build fails.

### Source Entries

Source entries can be local paths, `file://` paths, `http://` URLs, or `https://`
URLs.

Local relative paths are resolved relative to the build manifest directory.
Resolved sources are copied into `SOURCE_DIR` using the basename of the source
location.

A source entry may include a BLAKE2b-512 checksum suffix:

```yaml
source:
  - ./local-file.txt
  - ./checked-file.txt:abc123
  - https://example.com/archive.tar.gz:def456
```

The suffix is recognized as a checksum only when the text after the final colon
is non-empty hexadecimal. If no checksum suffix is present, source verification
is skipped.

### File Actions In Build Manifests

The build manifest `files` section is a compact way to generate archive
manifest install actions.

Each map key is an action target category. Each entry must use:

```text
<payload-relative-path>:<link|copy>
```

Example:

```yaml
files:
  bin:
    - bin/example:link
  icons:
    - share/icons/hicolor/128x128/apps/example.png:copy
  man:
    - share/man/man1/example.1:copy
```

Parcel validates that each declared source exists in `OUTPUT_DIR` after
`install_script` completes.

## Remote Repository Format

A Parcel remote serves a zstd-compressed JSON repository index named
`parcel-index.db`.

When a remote URL is added:

- If the URL already ends with `parcel-index.db`, Parcel uses it directly.
- If the URL contains `github.com/`, Parcel appends
  `/releases/download/parcel-index/parcel-index.db`.
- Otherwise, Parcel uses the URL as given.

The decoded JSON object has reserved top-level keys and package entries.

Required reserved keys:

- `_dl`: package download URL template.

Optional reserved keys:

- `_dl_delta`: delta package download URL template. Reserved; not implemented.

Package entries are keyed by package name.

Example:

```json
{
  "_dl": "https://example.com/releases/{name}-{version}-{arch}.parcel",
  "_dl_delta": "https://example.com/releases/{name}-{version}-{arch}.delta.parcel",
  "example": {
    "description": "Example parcel package",
    "homepage": "https://example.com",
    "versions": {
      "1.0.0-1": "0123456789abcdef"
    },
    "_delta": {}
  }
}
```

Package object fields:

- `description`: short package description.
- `homepage`: optional homepage URL.
- `versions`: map of version string to BLAKE2b-512 checksum of the `.parcel`
  archive bytes.
- `_delta`: reserved map for delta metadata.

Download templates currently expand:

- `{name}`: package name.
- `{version}`: full package version.
- `{arch}`: current machine architecture.

During remote install or update, Parcel downloads the selected package archive
and verifies it against the checksum stored in `versions`.

## User State

Parcel stores user state under:

```text
$HOME/.local/share/parcel/
```

Current layout:

```text
$HOME/.local/share/parcel/apps/<name>/<version>/
$HOME/.local/share/parcel/parcel.db
$HOME/.local/share/parcel/remotes.json
$HOME/.local/share/parcel/indexes/<remote>.db
```

`parcel.db` is a zstd-compressed JSON package database. It records installed
packages, install paths, applied actions, installed payload file paths, source
remote names, and install timestamps.

`remotes.json` is a pretty-printed JSON remote configuration file.

`indexes/<remote>.db` stores cached zstd-compressed JSON repository indexes.

## Security Rules

Parcel enforces these rules in the current implementation:

- Package payload paths must be relative and must not contain `..`.
- Build-manifest action sources must be relative and must not contain `..`.
- Install actions refuse to overwrite existing target paths.
- Remote package downloads are checked with BLAKE2b-512 when installed from a
  repository index.
- Build sources are checked with BLAKE2b-512 only when a checksum suffix is
  provided.

