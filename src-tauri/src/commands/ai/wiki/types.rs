use serde::{Deserialize, Serialize};

use crate::commands::wiki;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WikiGenerationBackend {
    Builtin {
        /// 模型引用(复合值 "providerId/modelId";None = 设置页默认模型)
        #[serde(default)]
        model: Option<String>,
        /// 思考强度(pi 七档 off..max;None = 模型默认:reasoning 中档 / 否则关闭)
        #[serde(default)]
        thinking: Option<String>,
        /// 页面并发数(None/0 = 沿用设置页全局 AI 并发,上限 8)
        #[serde(default)]
        concurrency: Option<usize>,
    },
    Agent {
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        custom_command: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        thinking: Option<String>,
        /// 页面并发数(agent 每页独立会话,可并行生成;None/0 = 默认 2,上限 8)
        #[serde(default)]
        concurrency: Option<usize>,
    },
}

impl Default for WikiGenerationBackend {
    fn default() -> Self {
        Self::Builtin {
            model: None,
            thinking: None,
            concurrency: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateWikiRequest {
    pub(crate) run_id: String,
    pub(crate) project_path: String,
    pub(crate) project_name: String,
    pub(crate) language: String,
    pub(crate) concurrency: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegenerateWikiPageRequest {
    pub(crate) run_id: String,
    pub(crate) project_path: String,
    pub(crate) language: String,
    pub(super) page: wiki::WikiOutlinePage,
    #[serde(default)]
    pub(super) changed_files: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWikiRequest {
    pub(crate) run_id: String,
    pub(crate) project_path: String,
    pub(crate) language: String,
    #[serde(default)]
    pub(crate) automatic: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegeneratedWikiPage {
    pub(super) model: String,
    pub(super) generator: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiUpdateEvent {
    pub(super) completed: usize,
    pub(super) total: usize,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiUpdateResult {
    pub(crate) updated_page_ids: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WikiGenerationEvent {
    Phase {
        phase: String,
    },
    Page {
        page: wiki::WikiOutlinePage,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// 该页生成耗时(毫秒,done 时上报)
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Progress {
        page_id: String,
        content: String,
    },
    Retry {
        #[serde(skip_serializing_if = "Option::is_none")]
        page_id: Option<String>,
        attempt: usize,
        max_attempts: usize,
        delay_seconds: u64,
        reason: String,
    },
    Context {
        file_count: usize,
        tree_truncated: bool,
        has_readme: bool,
        manifest_count: usize,
    },
    ActivityBatch {
        activity_type: String,
        items: Vec<String>,
    },
}
