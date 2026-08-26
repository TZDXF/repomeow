use std::time::Duration;

use agent_client_protocol::schema::v1::{
    NewSessionResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionConfigSelectOptions, SessionId, SetSessionConfigOptionRequest,
    SetSessionModeRequest,
};
use agent_client_protocol::{Agent, ConnectionTo};
use serde::Serialize;

use super::AcpConfigChoice;

const CONFIG_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigOptionInfo {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) category: Option<String>,
    pub(super) current: Option<String>,
    pub(super) choices: Vec<AcpConfigChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpModeInfo {
    pub(super) id: String,
    pub(super) name: String,
}

/// session/new 响应 → IPC 快照:config_options(仅 select 类)与旧式 modes
pub(super) fn session_options_snapshot(
    resp: &NewSessionResponse,
) -> (Vec<AcpConfigOptionInfo>, Vec<AcpModeInfo>) {
    let config_options = resp
        .config_options
        .iter()
        .flatten()
        .filter(|o| matches!(o.kind, SessionConfigKind::Select(_)))
        .map(config_option_info)
        .collect();
    let modes = resp
        .modes
        .as_ref()
        .map(|m| {
            m.available_modes
                .iter()
                .map(|mode| AcpModeInfo {
                    id: mode.id.to_string(),
                    name: mode.name.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    (config_options, modes)
}

pub(super) fn config_option_info(opt: &SessionConfigOption) -> AcpConfigOptionInfo {
    let (current, choices) = match &opt.kind {
        SessionConfigKind::Select(sel) => {
            let choices = match &sel.options {
                SessionConfigSelectOptions::Ungrouped(list) => list
                    .iter()
                    .map(|o| AcpConfigChoice {
                        id: o.value.to_string(),
                        name: o.name.clone(),
                    })
                    .collect(),
                SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|g| {
                        g.options.iter().map(|o| AcpConfigChoice {
                            id: o.value.to_string(),
                            name: o.name.clone(),
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            (Some(sel.current_value.to_string()), choices)
        }
        _ => (None, Vec::new()),
    };
    AcpConfigOptionInfo {
        id: opt.id.to_string(),
        name: opt.name.clone(),
        category: category_str(&opt.category),
        current,
        choices,
    }
}

pub(super) fn category_str(c: &Option<SessionConfigOptionCategory>) -> Option<String> {
    match c {
        Some(SessionConfigOptionCategory::Mode) => Some("mode".into()),
        Some(SessionConfigOptionCategory::Model) => Some("model".into()),
        Some(SessionConfigOptionCategory::ModelConfig) => Some("model_config".into()),
        Some(SessionConfigOptionCategory::ThoughtLevel) => Some("thought_level".into()),
        Some(SessionConfigOptionCategory::Other(s)) => Some(s.clone()),
        _ => None,
    }
}

/// 会话创建后应用模型/思考强度。无效值或请求失败只记录日志。
pub(super) async fn apply_session_config(
    conn: &ConnectionTo<Agent>,
    session_id: &SessionId,
    config_options: &[AcpConfigOptionInfo],
    modes: &[AcpModeInfo],
    model: Option<&str>,
    thinking: Option<&str>,
) {
    if let Some(value) = thinking {
        let opt = config_options
            .iter()
            .find(|o| o.category.as_deref() == Some("thought_level"));
        match opt {
            Some(opt) if opt.choices.iter().any(|c| c.id == value) => {
                set_config_option(conn, session_id, &opt.id, value).await;
            }
            _ => eprintln!("[agent] 未应用思考强度 {value:?}(agent 未上报该选项或不包含此值)"),
        }
    }
    if let Some(value) = model {
        let opt = config_options
            .iter()
            .find(|o| o.category.as_deref() == Some("model"));
        if let Some(opt) = opt {
            if opt.choices.iter().any(|c| c.id == value) {
                set_config_option(conn, session_id, &opt.id, value).await;
            } else {
                eprintln!("[agent] 未应用模型选择 {value:?}(不在 agent 上报的模型列表内)");
            }
        } else if modes.iter().any(|m| m.id == value) {
            let req = SetSessionModeRequest::new(session_id.clone(), value.to_string());
            match tokio::time::timeout(CONFIG_TIMEOUT, conn.send_request(req).block_task()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("[agent] set_mode 失败: {e}"),
                Err(_) => eprintln!("[agent] set_mode 超时"),
            }
        } else {
            eprintln!("[agent] 未应用模型选择 {value:?}(不在 agent 上报的模型/mode 列表内)");
        }
    }
}

async fn set_config_option(
    conn: &ConnectionTo<Agent>,
    session_id: &SessionId,
    config_id: &str,
    value: &str,
) {
    let req = SetSessionConfigOptionRequest::new(
        session_id.clone(),
        config_id.to_string(),
        SessionConfigOptionValue::value_id(value.to_string()),
    );
    match tokio::time::timeout(CONFIG_TIMEOUT, conn.send_request(req).block_task()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => eprintln!("[agent] set_config_option({config_id}) 失败: {e}"),
        Err(_) => eprintln!("[agent] set_config_option({config_id}) 超时"),
    }
}
