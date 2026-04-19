use std::time::Duration;

use reqwest::{Client, Url};
use serde_json::Value;

use crate::error::{AppError, Result};
use crate::lceda::models::SearchItem;
use crate::util::nested_string;

const SEARCH_API: &str = "https://pro.lceda.cn/api/szlcsc/eda/product/list";
const COMPONENT_API: &str = "https://pro.lceda.cn/api/components";
const STEP_API: &str = "https://modules.lceda.cn/qAxj6KHrDKw4blvCG8QJPs7Y";
const OBJ_API: &str = "https://modules.lceda.cn/3dmodel";

#[derive(Debug, Clone)]
pub struct LcedaClient {
    client: Client,
}

impl LcedaClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(format!("syft/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(35))
            .build()
            .expect("failed to build reqwest client");
        Self { client }
    }

    pub async fn search_components(&self, keyword: &str) -> Result<Vec<SearchItem>> {
        let mut url = Url::parse(SEARCH_API)
            .map_err(|err| AppError::InvalidResponse(format!("bad search url: {err}")))?;
        url.query_pairs_mut().append_pair("wd", keyword);

        let payload = self.get_json(url.as_str()).await?;
        let Some(results) = payload.get("result").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };

        let mut items = Vec::with_capacity(results.len());
        for (index, raw) in results.iter().enumerate() {
            let attrs = raw.get("attributes").unwrap_or(&Value::Null);
            items.push(SearchItem {
                index: index + 1,
                display_title: raw
                    .get("display_title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                title: raw
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                manufacturer: attrs
                    .get("Manufacturer")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                model_uuid: attrs
                    .get("3D Model")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                raw: raw.clone(),
            });
        }

        Ok(items)
    }

    pub async fn select_item(&self, keyword: &str, index: usize) -> Result<SearchItem> {
        let items = self.search_components(keyword).await?;
        if items.is_empty() {
            return Err(AppError::NoResults(keyword.to_string()));
        }
        if !(1..=items.len()).contains(&index) {
            return Err(AppError::InvalidIndex {
                keyword: keyword.to_string(),
                index,
                max: items.len(),
            });
        }
        Ok(items[index - 1].clone())
    }

    pub async fn component_detail(&self, uuid: &str) -> Result<Value> {
        let mut url = Url::parse(&format!("{COMPONENT_API}/{uuid}"))
            .map_err(|err| AppError::InvalidResponse(format!("bad component url: {err}")))?;
        url.query_pairs_mut().append_pair("uuid", uuid);
        self.get_json(url.as_str()).await
    }

    pub async fn get_model_uuid(&self, item: &SearchItem) -> Result<String> {
        let Some(seed_uuid) = item.model_uuid.as_deref() else {
            return Err(AppError::MissingModelUuid);
        };

        let detail = self.component_detail(seed_uuid).await?;
        let code = detail
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if code == 0 {
            if let Some(model_uuid) = nested_string(&detail, &["result", "3d_model_uuid"]) {
                return Ok(model_uuid);
            }
        }

        Ok(seed_uuid.to_string())
    }

    pub async fn download_step_bytes(&self, model_uuid: &str) -> Result<Vec<u8>> {
        let url = format!("{STEP_API}/{model_uuid}");
        self.get_bytes(&url).await
    }

    pub async fn download_obj_bytes(&self, model_uuid: &str) -> Result<Vec<u8>> {
        let url = format!("{OBJ_API}/{model_uuid}");
        self.get_bytes(&url).await
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        let response = self.client.get(url).send().await?.error_for_status()?;
        let bytes = response.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.client.get(url).send().await?.error_for_status()?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }
}

impl Default for LcedaClient {
    fn default() -> Self {
        Self::new()
    }
}
