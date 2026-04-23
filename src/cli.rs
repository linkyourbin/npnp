use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "npnp")]
#[command(version)]
#[command(about = "Normalize Pin Net Pad (npnp) - Pure Rust LCEDA downloader and bundle exporter")]
pub struct Cli {
    /// Show ready-to-run example commands
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub prompt: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
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
    /// Batch export Altium libraries from a text file of LCSC IDs
    Batch {
        #[arg(long, short = 'i', value_name = "FILE")]
        input: PathBuf,
        #[arg(long, default_value = "batch")]
        output: PathBuf,
        #[arg(long)]
        schlib: bool,
        #[arg(long)]
        pcblib: bool,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        merge: bool,
        #[arg(long)]
        append: bool,
        #[arg(long)]
        library_name: Option<String>,
        #[arg(long, default_value_t = 4)]
        parallel: usize,
        #[arg(long)]
        continue_on_error: bool,
        #[arg(long)]
        force: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prompt_without_subcommand() {
        let cli = Cli::try_parse_from(["npnp", "--prompt"]).expect("prompt flag should parse");
        assert!(cli.prompt);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_search_command_with_optional_subcommand_field() {
        let cli =
            Cli::try_parse_from(["npnp", "search", "C2040"]).expect("search command should parse");
        assert!(!cli.prompt);
        let Some(Commands::Search { keyword, limit }) = cli.command else {
            panic!("expected search command");
        };
        assert_eq!(keyword, "C2040");
        assert_eq!(limit, 20);
    }
}
