use std::ffi::OsStr;
use std::path::Path;

use clap::{CommandFactory, Parser};

use crate::batch::{BatchOptions, export_batch};
use crate::cli::{Cli, Commands};
use crate::error::Result;
use crate::lceda::LcedaClient;
use crate::workflow::{
    download_obj, download_step, export_bundle, export_easyeda_sources, export_pcblib,
    export_schlib,
};

pub async fn run_from_env() -> i32 {
    let invoked_as = std::env::args_os()
        .next()
        .as_deref()
        .map(display_invocation_name)
        .unwrap_or_else(|| "npnp".to_string());

    match run_cli(Cli::parse(), &invoked_as).await {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("Error: {err}");
            2
        }
    }
}

pub async fn run_cli(cli: Cli, invoked_as: &str) -> Result<()> {
    if cli.prompt {
        println!("{}", prompt_examples(invoked_as));
        return Ok(());
    }

    let Some(command) = cli.command else {
        let mut help = Cli::command();
        help.print_help()?;
        println!();
        return Ok(());
    };

    let client = LcedaClient::new();

    match command {
        Commands::Search { keyword, limit } => {
            let items = client.search_components(&keyword).await?;
            if items.is_empty() {
                println!("No results.");
                return Ok(());
            }

            let count = items.len().min(limit);
            println!("Found {} result(s), showing first {}:", items.len(), count);
            for item in items.iter().take(count) {
                let model_flag = if item.model_uuid.is_some() {
                    "yes"
                } else {
                    "no"
                };
                let lcsc_id = item.lcsc_id().unwrap_or_else(|| "-".to_string());
                let manufacturer = if item.manufacturer.is_empty() {
                    "-"
                } else {
                    item.manufacturer.as_str()
                };
                println!(
                    "[{:>3}] {} | LCSC ID: {} | Manufacturer: {} | 3D model: {}",
                    item.index,
                    item.display_name(),
                    lcsc_id,
                    manufacturer,
                    model_flag
                );
            }
        }
        Commands::DownloadStep {
            keyword,
            index,
            output,
            force,
        } => {
            let item = client.select_item(&keyword, index).await?;
            let path = download_step(&client, &item, &output, force).await?;
            println!("STEP saved: {}", path.display());
        }
        Commands::DownloadObj {
            keyword,
            index,
            output,
            force,
        } => {
            let item = client.select_item(&keyword, index).await?;
            let (obj_path, mtl_path) = download_obj(&client, &item, &output, force).await?;
            println!("OBJ saved: {}", obj_path.display());
            println!("MTL saved: {}", mtl_path.display());
        }
        Commands::ExportSource {
            keyword,
            index,
            output,
            force,
        } => {
            let item = client.select_item(&keyword, index).await?;
            let result = export_easyeda_sources(&client, &item, &output, force).await?;
            if let Some(path) = result.get("symbol") {
                println!("Symbol source saved: {}", path.display());
            }
            if let Some(path) = result.get("footprint") {
                println!("Footprint source saved: {}", path.display());
            }
        }
        Commands::ExportSchlib {
            keyword,
            index,
            output,
            force,
        } => {
            let item = client.select_item(&keyword, index).await?;
            let path = export_schlib(&client, &item, &output, force).await?;
            println!("SchLib saved: {}", path.display());
        }
        Commands::ExportPcblib {
            keyword,
            index,
            output,
            force,
        } => {
            let item = client.select_item(&keyword, index).await?;
            let path = export_pcblib(&client, &item, &output, force).await?;
            println!("PcbLib saved: {}", path.display());
        }
        Commands::Bundle {
            keyword,
            index,
            output,
            force,
        } => {
            let item = client.select_item(&keyword, index).await?;
            let result = export_bundle(&client, &item, &output, force).await?;
            if let Some(path) = result.get("manifest") {
                println!("Bundle manifest saved: {}", path.display());
            }
            if let Some(path) = result.get("symbol") {
                println!("Symbol source saved: {}", path.display());
            }
            if let Some(path) = result.get("footprint") {
                println!("Footprint source saved: {}", path.display());
            }
            if let Some(path) = result.get("step") {
                println!("STEP saved: {}", path.display());
            }
        }
        Commands::Batch {
            input,
            output,
            schlib,
            pcblib,
            full,
            merge,
            append,
            library_name,
            parallel,
            continue_on_error,
            force,
        } => {
            let summary = export_batch(
                &client,
                BatchOptions {
                    input,
                    output,
                    schlib,
                    pcblib,
                    full,
                    merge,
                    append,
                    library_name,
                    parallel,
                    continue_on_error,
                    force,
                },
            )
            .await?;
            println!(
                "Batch export complete. Total: {} | Skipped: {} | Success: {} | Failed: {}",
                summary.total, summary.skipped, summary.success, summary.failed
            );
            if !summary.failed_ids.is_empty() {
                println!("Failed IDs: {}", summary.failed_ids.join(", "));
            }
            for path in &summary.generated_files {
                println!("Generated: {}", path.display());
            }
            println!("Output directory: {}", summary.output.display());
        }
    }

    Ok(())
}

fn display_invocation_name(raw: &OsStr) -> String {
    Path::new(raw)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("npnp")
        .to_string()
}

fn prompt_examples(invoked_as: &str) -> String {
    let command = if invoked_as.trim().is_empty() {
        "npnp"
    } else {
        invoked_as.trim()
    };

    [
        "Normalize Pin Net Pad (npnp) ready-to-run commands:",
        "",
        "Search a component",
        &format!("  {command} search C2040 --limit 5"),
        "",
        "Export one schematic library",
        &format!("  {command} export-schlib C2040 --index 1 --output schlib --force"),
        "",
        "Export one PCB library",
        &format!("  {command} export-pcblib C2040 --index 1 --output pcblib --force"),
        "",
        "Export EasyEDA source JSON plus STEP bundle",
        &format!("  {command} bundle C2040 --index 1 --output bundle --force"),
        "",
        "Batch export both libraries from ids.txt",
        &format!(
            "  {command} batch --input ids.txt --output generated\\quick_check --full --force --continue-on-error"
        ),
        "",
        "Merge both libraries into one pair of outputs",
        &format!(
            "  {command} batch --input ids.txt --output generated\\merged --merge --library-name MyLib --full --continue-on-error"
        ),
        "",
]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{display_invocation_name, prompt_examples};
    use std::ffi::OsStr;

    #[test]
    fn prompt_examples_use_requested_command_name() {
        let text = prompt_examples("npnp");
        assert!(text.contains("npnp search C2040 --limit 5"));
        assert!(text.contains("npnp export-pcblib C2040 --index 1 --output pcblib --force"));
    }

    #[test]
    fn invocation_name_strips_exe_extension() {
        assert_eq!(
            display_invocation_name(OsStr::new(r"C:\tools\npnp.exe")),
            "npnp"
        );
    }
}
