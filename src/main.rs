use clap::Parser;
use syft::batch::{BatchOptions, export_batch};
use syft::cli::{Cli, Commands};
use syft::error::Result;
use syft::lceda::LcedaClient;
use syft::workflow::{
    download_obj, download_step, export_bundle, export_easyeda_sources, export_pcblib,
    export_schlib,
};

#[tokio::main]
async fn main() {
    let exit_code = match run().await {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("Error: {err}");
            2
        }
    };
    std::process::exit(exit_code);
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let client = LcedaClient::new();

    match cli.command {
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
                let manufacturer = if item.manufacturer.is_empty() {
                    "-"
                } else {
                    item.manufacturer.as_str()
                };
                println!(
                    "[{:>3}] {} | Manufacturer: {} | 3D model: {}",
                    item.index,
                    item.display_name(),
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
            println!("Output directory: {}", summary.output.display());
        }
    }

    Ok(())
}
