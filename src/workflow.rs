use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::to_string_pretty;

use crate::error::{AppError, Result};
use crate::footprint::build_pcblib_from_payload;
use crate::lceda::{LcedaClient, SearchItem};
use crate::pcblib::write_pcblib;
use crate::schlib::write_schlib_from_payload;
use crate::util::{nested_string, sanitize_filename, split_obj_and_mtl};

#[derive(Debug, Serialize)]
struct BundleManifest {
    component_name: String,
    manufacturer: String,
    search_index: usize,
    symbol_uuid: Option<String>,
    footprint_uuid: Option<String>,
    model_seed_uuid: Option<String>,
    model_resolved_uuid: Option<String>,
    symbol_file: Option<String>,
    footprint_file: Option<String>,
    step_file: Option<String>,
}

pub async fn download_step(
    client: &LcedaClient,
    item: &SearchItem,
    out_dir: &Path,
    force: bool,
) -> Result<PathBuf> {
    fs::create_dir_all(out_dir)?;
    let out_file = out_dir.join(item.choose_step_filename());
    if out_file.exists() && !force {
        return Ok(out_file);
    }

    let model_uuid = client.get_model_uuid(item).await?;
    let content = client.download_step_bytes(&model_uuid).await?;
    fs::write(&out_file, content)?;
    Ok(out_file)
}

pub async fn download_obj(
    client: &LcedaClient,
    item: &SearchItem,
    out_dir: &Path,
    force: bool,
) -> Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(out_dir)?;
    let base_name = item.choose_obj_basename();
    let obj_file = out_dir.join(format!("{base_name}.obj"));
    let mtl_file = out_dir.join(format!("{base_name}.mtl"));

    if obj_file.exists() && mtl_file.exists() && !force {
        return Ok((obj_file, mtl_file));
    }

    let model_uuid = client.get_model_uuid(item).await?;
    let content = client.download_obj_bytes(&model_uuid).await?;
    let text = String::from_utf8_lossy(&content);
    let (obj_text, mtl_text) = split_obj_and_mtl(&text);
    let obj_with_header = format!("mtllib {base_name}.mtl\n{obj_text}");

    fs::write(&obj_file, obj_with_header)?;
    fs::write(&mtl_file, mtl_text)?;
    Ok((obj_file, mtl_file))
}

pub async fn export_easyeda_sources(
    client: &LcedaClient,
    item: &SearchItem,
    out_dir: &Path,
    force: bool,
) -> Result<BTreeMap<String, PathBuf>> {
    fs::create_dir_all(out_dir)?;

    let base = sanitize_filename(item.display_name());
    let symbol_uuid = item.symbol_uuid();
    let footprint_uuid = item.footprint_uuid();
    if symbol_uuid.is_none() && footprint_uuid.is_none() {
        return Err(AppError::MissingSymbolOrFootprint);
    }

    let mut exported = BTreeMap::new();

    if let Some(symbol_uuid) = symbol_uuid {
        let symbol_data = client.component_detail(&symbol_uuid).await?;
        let symbol_file = out_dir.join(format!("{base}_symbol_easyeda.json"));
        if force || !symbol_file.exists() {
            fs::write(&symbol_file, to_string_pretty(&symbol_data)?)?;
        }
        exported.insert("symbol".to_string(), symbol_file);
    }

    if let Some(footprint_uuid) = footprint_uuid {
        let footprint_data = client.component_detail(&footprint_uuid).await?;
        let footprint_file = out_dir.join(format!("{base}_footprint_easyeda.json"));
        if force || !footprint_file.exists() {
            fs::write(&footprint_file, to_string_pretty(&footprint_data)?)?;
        }
        exported.insert("footprint".to_string(), footprint_file);
    }

    Ok(exported)
}

pub async fn export_pcblib(
    client: &LcedaClient,
    item: &SearchItem,
    out_dir: &Path,
    force: bool,
) -> Result<PathBuf> {
    fs::create_dir_all(out_dir)?;
    let footprint_uuid = item
        .footprint_uuid()
        .ok_or(AppError::MissingSymbolOrFootprint)?;
    let footprint_data = client.component_detail(&footprint_uuid).await?;
    let component_name = if item.display_name().trim().is_empty() {
        nested_string(&footprint_data, &["result", "display_title"])
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| item.display_name().to_string())
    } else {
        item.display_name().to_string()
    };
    let out_file = out_dir.join(format!("{}.PcbLib", sanitize_filename(&component_name)));
    if out_file.exists() && !force {
        return Ok(out_file);
    }

    let mut model_candidates = Vec::new();
    if let Some(model_uuid) = nested_string(&footprint_data, &["result", "model_3d", "uri"])
        .filter(|uuid| !uuid.trim().is_empty())
    {
        model_candidates.push(model_uuid);
    }
    if let Some(model_uuid) = item
        .model_uuid
        .clone()
        .filter(|uuid| !uuid.trim().is_empty())
    {
        if !model_candidates
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&model_uuid))
        {
            model_candidates.push(model_uuid);
        }
    }
    if !model_candidates.is_empty() {
        if let Ok(model_uuid) = client.get_model_uuid(item).await {
            if !model_candidates
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&model_uuid))
            {
                model_candidates.push(model_uuid);
            }
        }
    }

    let mut step_bytes = None;
    for model_uuid in model_candidates {
        if let Ok(bytes) = client.download_step_bytes(&model_uuid).await {
            step_bytes = Some(bytes);
            break;
        }
    }

    let library =
        build_pcblib_from_payload(&footprint_data, &component_name, step_bytes.as_deref())?;
    write_pcblib(&library, &out_file)?;
    Ok(out_file)
}

pub async fn export_schlib(
    client: &LcedaClient,
    item: &SearchItem,
    out_dir: &Path,
    force: bool,
) -> Result<PathBuf> {
    fs::create_dir_all(out_dir)?;
    let symbol_uuid = item
        .symbol_uuid()
        .ok_or(AppError::MissingSymbolOrFootprint)?;
    let symbol_data = client.component_detail(&symbol_uuid).await?;
    let component_name = if item.display_name().trim().is_empty() {
        nested_string(&symbol_data, &["result", "display_title"])
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| item.display_name().to_string())
    } else {
        item.display_name().to_string()
    };
    let out_file = out_dir.join(format!("{}.SchLib", sanitize_filename(&component_name)));
    if out_file.exists() && !force {
        return Ok(out_file);
    }

    write_schlib_from_payload(&symbol_data, &component_name, &out_file)?;
    Ok(out_file)
}

pub async fn export_bundle(
    client: &LcedaClient,
    item: &SearchItem,
    out_dir: &Path,
    force: bool,
) -> Result<BTreeMap<String, PathBuf>> {
    fs::create_dir_all(out_dir)?;

    let mut exported = export_easyeda_sources(client, item, out_dir, force).await?;
    let base = sanitize_filename(item.display_name());

    let mut resolved_model_uuid = None;
    let mut step_file = None;
    if item.model_uuid.is_some() {
        let model_uuid = client.get_model_uuid(item).await?;
        resolved_model_uuid = Some(model_uuid.clone());
        let path = out_dir.join(item.choose_step_filename());
        if force || !path.exists() {
            let content = client.download_step_bytes(&model_uuid).await?;
            fs::write(&path, content)?;
        }
        step_file = Some(path.clone());
        exported.insert("step".to_string(), path);
    }

    let manifest = BundleManifest {
        component_name: item.display_name().to_string(),
        manufacturer: item.manufacturer.clone(),
        search_index: item.index,
        symbol_uuid: item.symbol_uuid(),
        footprint_uuid: item.footprint_uuid(),
        model_seed_uuid: item.model_uuid.clone(),
        model_resolved_uuid: resolved_model_uuid,
        symbol_file: exported.get("symbol").map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }),
        footprint_file: exported.get("footprint").map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }),
        step_file: step_file.as_ref().map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }),
    };

    let manifest_file = out_dir.join(format!("{base}_bundle.json"));
    if force || !manifest_file.exists() {
        fs::write(&manifest_file, to_string_pretty(&manifest)?)?;
    }
    exported.insert("manifest".to_string(), manifest_file);

    Ok(exported)
}
