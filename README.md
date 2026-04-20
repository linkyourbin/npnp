# npnp

Normalize Pin Net Pad (`npnp`) is a pure Rust LCEDA/EasyEDA downloader and Altium library exporter.

`npnp` can:

- search LCEDA/LCSC components
- download 3D models as STEP or OBJ/MTL
- export EasyEDA symbol / footprint source JSON
- export Altium schematic libraries (`.SchLib`)
- export Altium PCB footprint libraries (`.PcbLib`)
- batch export libraries from a text file of LCSC IDs

This project is pure Rust. It does not require C#, .NET, or external DLLs for export.


## Requirements

- Rust toolchain
- Network access to LCEDA / EasyEDA APIs
- Windows is the primary tested environment

## Build

Build a release binary:

```powershell
cargo build --release
```

Run with Cargo:

```powershell
cargo run --quiet --bin npnp -- --help
```

Run the built binary directly:

```powershell
.\target\release\npnp.exe --help
```

Show ready-to-run example commands:

```powershell
.\target\release\npnp.exe --prompt
```

## Commands

Top-level commands:

- `search`
- `download-step`
- `download-obj`
- `export-source`
- `export-schlib`
- `export-pcblib`
- `bundle`
- `batch`

You can see help for any command with:

```powershell
cargo run --quiet --bin npnp -- <command> --help
```

## Quick Start

Search for a component:

```powershell
cargo run --quiet --bin npnp -- search C2040 --limit 5
```

Export one schematic library:

```powershell
cargo run --quiet --bin npnp -- export-schlib C2040 --output schlib --force
```

Export one PCB library:

```powershell
cargo run --quiet --bin npnp -- export-pcblib C2040 --output pcblib --force
```

Export both in batch from `ids.txt`:

```powershell
cargo run --quiet --bin npnp -- batch --input ids.txt --output out --schlib --pcblib --force
```

## Single-Component Usage

### `search`

Search by keyword, LCSC ID, or part name:

```powershell
cargo run --quiet --bin npnp -- search TYPE-C --limit 20
```

### `export-schlib`

Export a pure Rust Altium schematic library:

```powershell
cargo run --quiet --bin npnp -- export-schlib C2040 --index 1 --output schlib --force
```

Notes:

- `<KEYWORD>` can be an LCSC ID like `C2040` or a general search keyword
- `--index` selects the search result row to export
- output files are written into the directory passed to `--output`

### `export-pcblib`

Export a pure Rust Altium PCB library:

```powershell
cargo run --quiet --bin npnp -- export-pcblib C2040 --index 1 --output pcblib --force
```

Notes:

- if a valid STEP model exists, it will be embedded into the exported footprint
- if no usable STEP model is available, the 3D body is left empty

### `export-source`

Export only the raw EasyEDA source JSON files for symbol / footprint data.

Use this when you want to inspect the upstream payload before library generation.

### `bundle`

Export a mixed source bundle:

- EasyEDA symbol JSON
- EasyEDA footprint JSON
- STEP file when available
- manifest JSON

Example:

```powershell
cargo run --quiet --bin npnp -- bundle C2040 --index 1 --output bundle --force
```

### `download-step`

Download a STEP file for a selected component result.

### `download-obj`

Download OBJ and MTL files for a selected component result.

## Batch Usage

The `batch` command reads a text file and extracts all LCSC IDs in the form `C<number>`.

Example `ids.txt`:

```text
C2040
C12074
C569043
```

The parser is tolerant:

- it extracts IDs from arbitrary text
- it accepts both `C2040` and `c2040`
- it deduplicates IDs
- it preserves first-seen order

Example:

```powershell
cargo run --quiet --bin npnp -- batch --input ids.txt --output batch_out --schlib --pcblib --parallel 4 --continue-on-error --force
```

Merged library example:

```powershell
cargo run --quiet --bin npnp -- batch --input ids.txt --output batch_out --merge --library-name MyLib --schlib --pcblib --continue-on-error
```

This writes:

```text
batch_out/MyLib.SchLib
batch_out/MyLib.PcbLib
```

Important options:

- `--schlib` export only schematic libraries
- `--pcblib` export only PCB libraries
- `--full` export both targets
- `--merge` write one merged `.SchLib` and/or `.PcbLib` instead of one file per component
- `--library-name <NAME>` set the merged output filename prefix
- `--parallel <N>` number of concurrent export jobs
- `--continue-on-error` keep going if one ID fails
- `--force` ignore checkpoint skips and overwrite outputs

Non-merge batch output layout:

```text
batch_out/
  .checkpoint
  schlib/
  pcblib/
```

`.checkpoint` stores completed IDs so later runs can skip already exported parts unless `--force` is used.

Merge batch output layout:

```text
batch_out/
  MyLib.SchLib
  MyLib.PcbLib
```

## Output Notes

### SchLib

Current schematic export includes:

- symbol body and graphics
- pins
- multipart symbols when present in source data
- metadata records such as `Designator`, `Comment`, `Description`, and parameters

### PcbLib

Current footprint export includes:

- pads and footprint primitives
- 3D STEP embedding when available

## Typical Workflow

1. Search the component.
2. Confirm the result index if you searched by a broad keyword.
3. Export `.SchLib`, `.PcbLib`, or both.
4. Open the output in Altium for verification.
5. Use `batch` when you already have a list of LCSC IDs.

## Examples

Search and export the first result:

```powershell
cargo run --quiet --bin npnp -- search RP2040 --limit 5
cargo run --quiet --bin npnp -- export-schlib RP2040 --index 1 --output out\schlib --force
cargo run --quiet --bin npnp -- export-pcblib RP2040 --index 1 --output out\pcblib --force
```

Batch export both libraries from `ids.txt`:

```powershell
cargo run --quiet --bin npnp -- batch --input ids.txt --output generated\check --schlib --pcblib --force --continue-on-error
```

## Troubleshooting

- If batch export says no valid IDs were found, check that the input file contains values like `C2040`.
- If a broad keyword gives the wrong part, use `search` first and then pass the correct `--index`.
- If a footprint opens without a 3D body, the upstream STEP model may be missing or unusable.
- Re-run with `--force` if you want to overwrite existing outputs or ignore the batch checkpoint.