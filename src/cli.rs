use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "syft")]
#[command(version)]
#[command(about = "Pure Rust LCEDA downloader and bundle exporter")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Search components by keyword
    Search {
        /// Search keyword, e.g. C8755 or TYPE-C
        keyword: String,
        /// Maximum result rows to print
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Search by keyword and download STEP by result index
    DownloadStep {
        keyword: String,
        #[arg(long, default_value_t = 1)]
        index: usize,
        #[arg(long, default_value = "step")]
        output: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Search by keyword and download OBJ/MTL by result index
    DownloadObj {
        keyword: String,
        #[arg(long, default_value_t = 1)]
        index: usize,
        #[arg(long, default_value = "obj")]
        output: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Export EasyEDA symbol / footprint JSON sources only
    ExportSource {
        keyword: String,
        #[arg(long, default_value_t = 1)]
        index: usize,
        #[arg(long, default_value = "easyeda_src")]
        output: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Export a pure Rust Altium schematic library (.SchLib)
    #[command(name = "export-schlib")]
    ExportSchlib {
        keyword: String,
        #[arg(long, default_value_t = 1)]
        index: usize,
        #[arg(long, default_value = "schlib")]
        output: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Export a pure Rust Altium PCB footprint library (.PcbLib)
    #[command(name = "export-pcblib")]
    ExportPcblib {
        keyword: String,
        #[arg(long, default_value_t = 1)]
        index: usize,
        #[arg(long, default_value = "pcblib")]
        output: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Export a pure-Rust input bundle: sources + STEP + manifest
    Bundle {
        keyword: String,
        #[arg(long, default_value_t = 1)]
        index: usize,
        #[arg(long, default_value = "bundle")]
        output: PathBuf,
        #[arg(long)]
        force: bool,
    },
}
