use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::error::{AppError, Result};
use crate::lceda::{LcedaClient, SearchItem};
use crate::pcblib::{PcbLibrary, write_pcblib};
use crate::schlib::{Component, write_schlib_library};
use crate::util::sanitize_filename;
use crate::workflow::{
    build_pcblib_library_for_item, build_schlib_component_for_item, export_pcblib, export_schlib,
};

#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub schlib: bool,
    pub pcblib: bool,
    pub full: bool,
    pub merge: bool,
    pub library_name: Option<String>,
    pub parallel: usize,
    pub continue_on_error: bool,
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct BatchSummary {
    pub total: usize,
    pub skipped: usize,
    pub success: usize,
    pub failed: usize,
    pub failed_ids: Vec<String>,
    pub output: PathBuf,
    pub generated_files: Vec<PathBuf>,
}

pub async fn export_batch(client: &LcedaClient, options: BatchOptions) -> Result<BatchSummary> {
    let targets = ExportTargets::resolve(&options)?;
    if options.merge {
        return export_batch_merged(client, options, targets).await;
    }

    let options = Arc::new(options);

    fs::create_dir_all(&options.output)?;

    let input = fs::read_to_string(&options.input)?;
    let ids = parse_lcsc_ids(&input);
    if ids.is_empty() {
        return Err(AppError::Other(
            "no valid LCSC IDs found in batch input".to_string(),
        ));
    }

    let checkpoint_path = options.output.join(".checkpoint");
    let completed = load_checkpoint(&checkpoint_path)?;

    let mut pending = Vec::new();
    let mut skipped = 0usize;
    for id in ids {
        if !options.force && completed.contains(&id) {
            skipped += 1;
        } else {
            pending.push(id);
        }
    }

    let mut summary = BatchSummary {
        total: pending.len() + skipped,
        skipped,
        success: 0,
        failed: 0,
        failed_ids: Vec::new(),
        output: options.output.clone(),
        generated_files: Vec::new(),
    };

    if pending.is_empty() {
        return Ok(summary);
    }

    if options.parallel > 1 && pending.len() > 1 {
        run_parallel(
            client.clone(),
            options.clone(),
            targets,
            &checkpoint_path,
            pending,
            &mut summary,
        )
        .await?;
    } else {
        run_sequential(
            client.clone(),
            options.clone(),
            targets,
            &checkpoint_path,
            pending,
            &mut summary,
        )
        .await?;
    }

    Ok(summary)
}

#[derive(Debug, Clone, Copy)]
struct ExportTargets {
    schlib: bool,
    pcblib: bool,
}

impl ExportTargets {
    fn resolve(options: &BatchOptions) -> Result<Self> {
        if options.parallel == 0 {
            return Err(AppError::Other("--parallel must be at least 1".to_string()));
        }

        let schlib = options.schlib || options.full;
        let pcblib = options.pcblib || options.full;
        if !schlib && !pcblib {
            return Err(AppError::Other(
                "at least one export target must be selected (--schlib, --pcblib, or --full)"
                    .to_string(),
            ));
        }

        Ok(Self { schlib, pcblib })
    }
}

#[derive(Debug)]
struct MergeArtifacts {
    schlib_component: Option<Component>,
    pcblib_library: Option<PcbLibrary>,
}

async fn export_batch_merged(
    client: &LcedaClient,
    options: BatchOptions,
    targets: ExportTargets,
) -> Result<BatchSummary> {
    fs::create_dir_all(&options.output)?;

    let input = fs::read_to_string(&options.input)?;
    let ids = parse_lcsc_ids(&input);
    if ids.is_empty() {
        return Err(AppError::Other(
            "no valid LCSC IDs found in batch input".to_string(),
        ));
    }

    let mut summary = BatchSummary {
        total: ids.len(),
        skipped: 0,
        success: 0,
        failed: 0,
        failed_ids: Vec::new(),
        output: options.output.clone(),
        generated_files: Vec::new(),
    };

    let library_name = resolve_library_name(&options);
    let mut used_names = HashSet::new();
    let mut schlib_components = Vec::new();
    let mut pcblib_library = PcbLibrary::default();
    let mut first_error = None;

    for id in ids {
        let result = export_merged_component(client, targets, &id, &mut used_names).await;
        match result {
            Ok(artifacts) => {
                if let Some(component) = artifacts.schlib_component {
                    schlib_components.push(component);
                }
                if let Some(library) = artifacts.pcblib_library {
                    append_pcblib_library(&mut pcblib_library, library);
                }
                summary.success += 1;
                println!("OK {id}");
            }
            Err(err) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                eprintln!("FAILED {id}: {err}");
                if first_error.is_none() {
                    first_error = Some(err);
                }
                if !options.continue_on_error {
                    return Err(first_error.unwrap());
                }
            }
        }
    }

    let mut wrote_any = false;
    if targets.schlib && !schlib_components.is_empty() {
        let path = options
            .output
            .join(format!("{}.SchLib", sanitize_filename(&library_name)));
        write_schlib_library(&schlib_components, &path)?;
        summary.generated_files.push(path);
        wrote_any = true;
    }
    if targets.pcblib && !pcblib_library.components.is_empty() {
        let path = options
            .output
            .join(format!("{}.PcbLib", sanitize_filename(&library_name)));
        write_pcblib(&pcblib_library, &path)?;
        summary.generated_files.push(path);
        wrote_any = true;
    }

    if !wrote_any {
        return Err(first_error.unwrap_or_else(|| {
            AppError::Other("no components exported successfully for merged batch".to_string())
        }));
    }

    Ok(summary)
}

async fn export_merged_component(
    client: &LcedaClient,
    targets: ExportTargets,
    lcsc_id: &str,
    used_names: &mut HashSet<String>,
) -> Result<MergeArtifacts> {
    let item = client.select_item(lcsc_id, 1).await?;
    let component_name = merged_component_name(&item, lcsc_id, used_names);

    let schlib_component = if targets.schlib {
        Some(build_schlib_component_for_item(client, &item, &component_name).await?)
    } else {
        None
    };

    let pcblib_library = if targets.pcblib {
        Some(build_pcblib_library_for_item(client, &item, &component_name).await?)
    } else {
        None
    };

    Ok(MergeArtifacts {
        schlib_component,
        pcblib_library,
    })
}

fn merged_component_name(
    item: &SearchItem,
    lcsc_id: &str,
    used_names: &mut HashSet<String>,
) -> String {
    let base = item.display_name().trim();
    let base = if base.is_empty() { lcsc_id } else { base };
    let normalized_base = base.to_ascii_lowercase();
    if used_names.insert(normalized_base) {
        return base.to_string();
    }

    let with_id = format!("{base}_{lcsc_id}");
    let normalized_with_id = with_id.to_ascii_lowercase();
    if used_names.insert(normalized_with_id) {
        return with_id;
    }

    let mut index = 2usize;
    loop {
        let candidate = format!("{base}_{lcsc_id}_{index}");
        if used_names.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        index += 1;
    }
}

fn append_pcblib_library(target: &mut PcbLibrary, source: PcbLibrary) {
    target.components.extend(source.components);
    target.models.extend(source.models);
}

fn resolve_library_name(options: &BatchOptions) -> String {
    if let Some(name) = options.library_name.as_deref() {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    options
        .input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("MergedLib")
        .to_string()
}

async fn run_sequential(
    client: LcedaClient,
    options: Arc<BatchOptions>,
    targets: ExportTargets,
    checkpoint_path: &Path,
    pending: Vec<String>,
    summary: &mut BatchSummary,
) -> Result<()> {
    for id in pending {
        match export_component(&client, &options, targets, &id).await {
            Ok(()) => {
                append_checkpoint(checkpoint_path, &id)?;
                summary.success += 1;
                println!("OK {id}");
            }
            Err(err) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                eprintln!("FAILED {id}: {err}");
                if !options.continue_on_error {
                    return Err(err);
                }
            }
        }
    }

    Ok(())
}

async fn run_parallel(
    client: LcedaClient,
    options: Arc<BatchOptions>,
    targets: ExportTargets,
    checkpoint_path: &Path,
    pending: Vec<String>,
    summary: &mut BatchSummary,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(options.parallel));
    let mut join_set = JoinSet::new();

    for id in pending {
        let client = client.clone();
        let options = options.clone();
        let semaphore = semaphore.clone();
        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("batch semaphore should remain open");
            let result = export_component(&client, &options, targets, &id).await;
            (id, result)
        });
    }

    let mut first_error = None;
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok((id, Ok(()))) => {
                append_checkpoint(checkpoint_path, &id)?;
                summary.success += 1;
                println!("OK {id}");
            }
            Ok((id, Err(err))) => {
                summary.failed += 1;
                summary.failed_ids.push(id.clone());
                eprintln!("FAILED {id}: {err}");
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
            Err(err) => {
                summary.failed += 1;
                let batch_err = AppError::Other(format!("batch task join failed: {err}"));
                eprintln!("FAILED batch task: {batch_err}");
                if first_error.is_none() {
                    first_error = Some(batch_err);
                }
            }
        }
    }

    if summary.failed > 0 && !options.continue_on_error {
        return Err(first_error.unwrap_or_else(|| AppError::Other("batch export failed".into())));
    }

    Ok(())
}

async fn export_component(
    client: &LcedaClient,
    options: &BatchOptions,
    targets: ExportTargets,
    lcsc_id: &str,
) -> Result<()> {
    let item = client.select_item(lcsc_id, 1).await?;

    if targets.schlib {
        let schlib_dir = options.output.join("schlib");
        export_schlib(client, &item, &schlib_dir, options.force).await?;
    }

    if targets.pcblib {
        let pcblib_dir = options.output.join("pcblib");
        export_pcblib(client, &item, &pcblib_dir, options.force).await?;
    }

    Ok(())
}

fn load_checkpoint(path: &Path) -> Result<HashSet<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(err) => Err(err.into()),
    }
}

fn append_checkpoint(path: &Path, id: &str) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{id}")?;
    Ok(())
}

fn parse_lcsc_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'C' || byte == b'c' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }

            if end > start {
                let digits = std::str::from_utf8(&bytes[start..end])
                    .expect("ASCII digits must be valid UTF-8");
                let id = format!("C{digits}");
                if seen.insert(id.clone()) {
                    ids.push(id);
                }
                index = end;
                continue;
            }
        }

        index += 1;
    }

    ids
}

#[cfg(test)]
mod tests {
    use super::{BatchOptions, parse_lcsc_ids, resolve_library_name};
    use std::path::PathBuf;

    #[test]
    fn parse_ids_deduplicates_and_preserves_order() {
        let ids = parse_lcsc_ids("C2040\nfoo C5676243 bar c2040 baz C42");
        assert_eq!(ids, vec!["C2040", "C5676243", "C42"]);
    }

    #[test]
    fn parse_ids_ignores_invalid_matches() {
        let ids = parse_lcsc_ids("C abc c-1 test");
        assert!(ids.is_empty());
    }

    #[test]
    fn resolve_library_name_defaults_to_input_stem() {
        let options = BatchOptions {
            input: PathBuf::from("ids.txt"),
            output: PathBuf::from("out"),
            schlib: true,
            pcblib: false,
            full: false,
            merge: true,
            library_name: None,
            parallel: 1,
            continue_on_error: false,
            force: false,
        };

        assert_eq!(resolve_library_name(&options), "ids");
    }
}
