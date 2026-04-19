use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::error::{AppError, Result};
use crate::lceda::LcedaClient;
use crate::workflow::{export_pcblib, export_schlib};

#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub schlib: bool,
    pub pcblib: bool,
    pub full: bool,
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
}

pub async fn export_batch(client: &LcedaClient, options: BatchOptions) -> Result<BatchSummary> {
    let targets = ExportTargets::resolve(&options)?;
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
        if completed.contains(&id) {
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
    use super::parse_lcsc_ids;

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
}
