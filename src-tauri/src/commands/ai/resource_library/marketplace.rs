//! skills.sh 市场适配：目录浏览、搜索与单个 Skill 的受限下载。
//!
//! skills.sh 未提供排行榜 JSON API，因此「全部 / 趋势 / 热门」从其公开 SSR 页面提取
//! 可识别的卡片链接；关键词搜索使用其稳定的 `/api/search` 接口。所有网络和导入输入
//! 均在本模块收口，避免远程内容直接影响资源库路径或元数据。

use std::collections::HashSet;
use std::io::Read;
use std::time::Duration;

use regex::Regex;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;

use super::errors::{codes, RlError, RlResult};
use super::models::{MarketplaceList, MarketplaceSkill, MarketplaceSource};

const SKILLS_HOST: &str = "https://skills.sh";
const MAX_CATALOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SKILL_BYTES: u64 = 512 * 1024;
const MAX_RESULTS: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    #[serde(default)]
    skills: Vec<SearchItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchItem {
    id: String,
    #[serde(default)]
    skill_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    installs: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadResponse {
    #[serde(default)]
    content: String,
    #[serde(default)]
    skill: Option<DownloadSkill>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadSkill {
    #[serde(default)]
    content: String,
    #[serde(default)]
    markdown: String,
}

fn client() -> RlResult<Client> {
    Client::builder()
        .user_agent("RepoMeow skills marketplace")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|err| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, err.to_string()))
}

fn read_limited(response: reqwest::blocking::Response, max: u64) -> RlResult<Vec<u8>> {
    if response.content_length().is_some_and(|length| length > max) {
        return Err(RlError::coded(
            codes::MARKETPLACE_RESPONSE_TOO_LARGE,
            "content-length",
        ));
    }
    let mut out = Vec::new();
    response
        .take(max + 1)
        .read_to_end(&mut out)
        .map_err(|err| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, err.to_string()))?;
    if out.len() as u64 > max {
        return Err(RlError::coded(
            codes::MARKETPLACE_RESPONSE_TOO_LARGE,
            "body",
        ));
    }
    Ok(out)
}

fn fetch_text(url: &str, max: u64) -> RlResult<String> {
    let response = client()?
        .get(url)
        .send()
        .map_err(|err| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, err.to_string()))?;
    if !response.status().is_success() {
        return Err(RlError::coded(
            codes::MARKETPLACE_UNAVAILABLE,
            format!("{} {url}", response.status()),
        ));
    }
    String::from_utf8(read_limited(response, max)?)
        .map_err(|err| RlError::coded(codes::MARKETPLACE_INVALID_RESPONSE, err.to_string()))
}

fn validate_part(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 100
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// 市场条目 ID 固定为 `<owner>/<repo>/<skill>`，拒绝 URL、空段与路径穿越。
pub(super) fn parse_marketplace_id(id: &str) -> RlResult<(&str, &str, &str)> {
    let mut parts = id.split('/');
    let (Some(owner), Some(repo), Some(slug), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(RlError::coded(codes::MARKETPLACE_ID_INVALID, id));
    };
    if [owner, repo, slug].into_iter().all(validate_part) {
        Ok((owner, repo, slug))
    } else {
        Err(RlError::coded(codes::MARKETPLACE_ID_INVALID, id))
    }
}

fn marketplace_url(id: &str) -> String {
    format!("{SKILLS_HOST}/{id}")
}

fn skill_from_parts(
    source: &str,
    skill_id: &str,
    name: String,
    installs: u64,
) -> Option<MarketplaceSkill> {
    let (owner, repo) = source.split_once('/')?;
    if source.matches('/').count() != 1 {
        return None;
    }
    let id = format!("{owner}/{repo}/{skill_id}");
    let (_, _, slug) = parse_marketplace_id(&id).ok()?;
    Some(MarketplaceSkill {
        id: id.clone(),
        name: if name.is_empty() {
            slug.to_string()
        } else {
            name
        },
        source: format!("{owner}/{repo}"),
        installs,
        url: marketplace_url(&id),
        installed_skill_id: None,
    })
}

fn from_search(item: SearchItem) -> Option<MarketplaceSkill> {
    if !item.id.is_empty() {
        let Ok((owner, repo, slug)) = parse_marketplace_id(&item.id) else {
            return None;
        };
        return skill_from_parts(&format!("{owner}/{repo}"), slug, item.name, item.installs);
    }
    skill_from_parts(&item.source, &item.skill_id, item.name, item.installs)
}

fn skills_from_array(items: &[Value]) -> Vec<MarketplaceSkill> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter_map(|item| {
            let source = item.get("source")?.as_str()?;
            let skill_id = item
                .get("skillId")
                .or_else(|| item.get("skill_id"))
                .or_else(|| item.get("id"))?
                .as_str()?;
            let key = format!("{source}/{skill_id}");
            if !seen.insert(key) {
                return None;
            }
            skill_from_parts(
                source,
                skill_id,
                item.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                item.get("installs")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            )
        })
        .take(MAX_RESULTS)
        .collect()
}

/// 关键词搜索走 skills.sh 唯一公开且结构化的目录接口。
pub(super) fn search(query: &str, source: Option<&str>) -> RlResult<MarketplaceList> {
    let query = query.trim();
    if query.is_empty() {
        return Err(RlError::coded(codes::MARKETPLACE_QUERY_REQUIRED, ""));
    }
    let mut request = client()?
        .get(format!("{SKILLS_HOST}/api/search"))
        .query(&[("q", query), ("limit", "100")]);
    if let Some(source) = source.filter(|value| !value.trim().is_empty()) {
        let Some((owner, repo)) = source.trim().split_once('/') else {
            return Err(RlError::coded(codes::MARKETPLACE_SOURCE_INVALID, source));
        };
        if !validate_part(owner) || !validate_part(repo) || source.matches('/').count() != 1 {
            return Err(RlError::coded(codes::MARKETPLACE_SOURCE_INVALID, source));
        }
        request = request.query(&[("owner", source.trim())]);
    }
    let response = request
        .send()
        .map_err(|err| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, err.to_string()))?;
    if !response.status().is_success() {
        return Err(RlError::coded(
            codes::MARKETPLACE_UNAVAILABLE,
            response.status().to_string(),
        ));
    }
    let bytes = read_limited(response, MAX_CATALOG_BYTES)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|err| RlError::coded(codes::MARKETPLACE_INVALID_RESPONSE, err.to_string()))?;
    let skills = if let Some(items) = value.get("skills").and_then(Value::as_array) {
        skills_from_array(items)
    } else if let Some(items) = value.as_array() {
        skills_from_array(items)
    } else {
        let data: SearchResponse = serde_json::from_value(value)
            .map_err(|err| RlError::coded(codes::MARKETPLACE_INVALID_RESPONSE, err.to_string()))?;
        data.skills
            .into_iter()
            .filter_map(from_search)
            .take(MAX_RESULTS)
            .collect()
    };
    Ok(MarketplaceList { skills })
}

/// 优先读取传统 Next.js 数据脚本；页面升级为 RSC 后会自然落到下一层解析。
fn parse_catalog_next_data(html: &str) -> Vec<MarketplaceSkill> {
    const MARKER: &str = r#"<script id="__NEXT_DATA__" type="application/json">"#;
    let Some(start) = html.find(MARKER).map(|index| index + MARKER.len()) else {
        return Vec::new();
    };
    let Some(end) = html[start..].find("</script>").map(|index| start + index) else {
        return Vec::new();
    };
    let Ok(data) = serde_json::from_str::<Value>(&html[start..end]) else {
        return Vec::new();
    };
    [
        "/props/pageProps/initialSkills",
        "/props/pageProps/skills",
        "/props/pageProps/items",
    ]
    .into_iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_array))
    .map_or_else(Vec::new, |items| skills_from_array(items))
}

fn skill_from_capture(
    source: &str,
    skill_id: &str,
    name: Option<&str>,
    installs: Option<&str>,
) -> Option<MarketplaceSkill> {
    skill_from_parts(
        &source.replace(r#"\""#, "\""),
        &skill_id.replace(r#"\""#, "\""),
        name.unwrap_or_default().replace(r#"\""#, "\""),
        installs
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default(),
    )
}

/// 兼容 Next.js RSC 的转义 JSON 片段以及旧版直接嵌入的对象。
fn parse_embedded_catalog(html: &str) -> Vec<MarketplaceSkill> {
    let primary = Regex::new(
        r#"(?:\\)?\"source(?:\\)?\":(?:\\)?\"(?P<source>[^\"\\]+)(?:\\)?\"(?:[^{}]|\\.)*?(?:(?:\\)?\"skillId(?:\\)?\"|(?:\\)?\"skill_id(?:\\)?\"):(?:\\)?\"(?P<skill_id>[^\"\\]+)(?:\\)?\"(?:[^{}]|\\.)*?(?:\\)?\"name(?:\\)?\":(?:\\)?\"(?P<name>[^\"\\]*)(?:\\)?\"(?:[^{}]|\\.)*?(?:\\)?\"installs(?:\\)?\":(?P<installs>\d+)"#,
    )
    .expect("RSC 市场对象正则固定有效");
    let legacy = Regex::new(
        r#"\{\"source\":\"(?P<source>[^\"]+)\",\"skill_id\":\"(?P<skill_id>[^\"]+)\"(?:,\"name\":\"(?P<name>[^\"]*)\")?(?:.*?\"installs\":(?P<installs>\d+))?\}"#,
    )
    .expect("旧市场对象正则固定有效");
    let mut seen = HashSet::new();
    primary
        .captures_iter(html)
        .chain(legacy.captures_iter(html))
        .filter_map(|capture| {
            let skill = skill_from_capture(
                &capture["source"],
                &capture["skill_id"],
                capture.name("name").map(|item| item.as_str()),
                capture.name("installs").map(|item| item.as_str()),
            )?;
            seen.insert(skill.id.clone()).then_some(skill)
        })
        .take(MAX_RESULTS)
        .collect()
}

/// 最后的兼容层只提取页面链接，因此无法读取安装量。
fn parse_catalog_urls(html: &str) -> Vec<MarketplaceSkill> {
    let re = Regex::new(
        r#"(?:https?://skills\.sh)?/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)"#,
    )
    .expect("市场链接正则固定有效");
    let mut ids = HashSet::new();
    re.captures_iter(html)
        .filter_map(|capture| {
            let id = format!("{}/{}/{}", &capture[1], &capture[2], &capture[3]);
            if parse_marketplace_id(&id).is_ok() {
                Some(id)
            } else {
                None
            }
        })
        .filter(|id| ids.insert(id.clone()))
        .take(MAX_RESULTS)
        .filter_map(|id| {
            let (owner, repo, slug) = parse_marketplace_id(&id).ok()?;
            skill_from_parts(&format!("{owner}/{repo}"), slug, String::new(), 0)
        })
        .collect()
}

/// 榜单先取能携带真实 installs 的结构化数据，最后才降级为链接提取。
fn parse_catalog_page(html: &str) -> Vec<MarketplaceSkill> {
    let next_data = parse_catalog_next_data(html);
    if !next_data.is_empty() {
        return next_data;
    }
    let embedded = parse_embedded_catalog(html);
    if !embedded.is_empty() {
        return embedded;
    }
    parse_catalog_urls(html)
}

pub(super) fn browse(mode: &str) -> RlResult<MarketplaceList> {
    let path = match mode {
        "all" => "",
        "trending" => "/trending",
        "hot" => "/hot",
        _ => return Err(RlError::coded(codes::MARKETPLACE_MODE_INVALID, mode)),
    };
    let skills = parse_catalog_page(&fetch_text(
        &format!("{SKILLS_HOST}{path}"),
        MAX_CATALOG_BYTES,
    )?);
    if skills.is_empty() {
        return Err(RlError::coded(
            codes::MARKETPLACE_INVALID_RESPONSE,
            "目录页面中没有可识别条目",
        ));
    }
    Ok(MarketplaceList { skills })
}

/// 下载单个市场 Skill。接口响应形状有过演进，兼容顶层 content 与嵌套 skill 内容，
/// 但始终只接受一个 UTF-8 SKILL.md 字符串。
pub(super) fn download(id: &str) -> RlResult<(MarketplaceSource, String)> {
    let (owner, repo, slug) = parse_marketplace_id(id)?;
    let url = format!("{SKILLS_HOST}/api/download/{owner}/{repo}/{slug}");
    let response = client()?
        .get(url)
        .send()
        .map_err(|err| RlError::coded(codes::MARKETPLACE_UNAVAILABLE, err.to_string()))?;
    if !response.status().is_success() {
        return Err(RlError::coded(
            codes::MARKETPLACE_UNAVAILABLE,
            response.status().to_string(),
        ));
    }
    let bytes = read_limited(response, MAX_SKILL_BYTES)?;
    let data: DownloadResponse = serde_json::from_slice(&bytes)
        .map_err(|err| RlError::coded(codes::MARKETPLACE_INVALID_RESPONSE, err.to_string()))?;
    let content = if !data.content.is_empty() {
        data.content
    } else if let Some(skill) = data.skill {
        if !skill.content.is_empty() {
            skill.content
        } else {
            skill.markdown
        }
    } else {
        String::new()
    };
    if content.trim().is_empty() {
        return Err(RlError::coded(
            codes::MARKETPLACE_INVALID_RESPONSE,
            "下载结果没有 SKILL.md",
        ));
    }
    Ok((
        MarketplaceSource {
            id: format!("{owner}/{repo}/{slug}"),
            source: format!("{owner}/{repo}"),
            url: marketplace_url(&format!("{owner}/{repo}/{slug}")),
        },
        content,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_three_safe_identifier_segments() {
        assert_eq!(
            parse_marketplace_id("owner/repo/a-skill").unwrap(),
            ("owner", "repo", "a-skill")
        );
        for invalid in [
            "owner/repo",
            "owner/repo/../../x",
            "https://x/y/z",
            "owner/repo/a skill",
        ] {
            assert!(parse_marketplace_id(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn parses_next_data_and_preserves_installs() {
        let entries = parse_catalog_page(
            r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"initialSkills":[{"source":"anthropics/skills","skillId":"template-skill","name":"Template Skill","installs":238},{"source":"vercel/ai","skillId":"ai-sdk","installs":265}]}}}</script>"#,
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "anthropics/skills/template-skill");
        assert_eq!(entries[0].installs, 238);
        assert_eq!(entries[1].name, "ai-sdk");
        assert_eq!(entries[1].installs, 265);
    }

    #[test]
    fn parses_rsc_payload_and_deduplicates() {
        let entries = parse_catalog_page(
            r#"self.__next_f.push([1,"[{\"source\":\"acme/tools\",\"skillId\":\"review\",\"name\":\"Code Review\",\"installs\":42},{\"source\":\"acme/tools\",\"skillId\":\"review\",\"name\":\"duplicate\",\"installs\":1}]"])"#,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "acme/tools/review");
        assert_eq!(entries[0].name, "Code Review");
        assert_eq!(entries[0].installs, 42);
    }

    #[test]
    fn parses_legacy_embedded_payload() {
        let entries = parse_catalog_page(
            r#"{"source":"openai/skills","skill_id":"playwright","name":"Playwright","installs":2}"#,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "openai/skills/playwright");
        assert_eq!(entries[0].installs, 2);
    }

    #[test]
    fn extracts_unique_catalog_links_as_last_resort() {
        let entries = parse_catalog_page(
            r#"<a href="https://skills.sh/acme/tools/code-review">one</a>
               <a href="https://skills.sh/acme/tools/code-review">two</a>
               <a href="https://skills.sh/vercel/react/ui">three</a>"#,
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "acme/tools/code-review");
        assert_eq!(entries[1].source, "vercel/react");
        assert_eq!(entries[0].installs, 0);
    }

    #[test]
    fn maps_array_items_with_snake_or_camel_case_skill_ids() {
        let data: Value = serde_json::from_str(
            r#"[{"source":"a/b","skillId":"camel","installs":3},{"source":"a/b","skill_id":"snake","installs":4}]"#,
        )
        .unwrap();
        let entries = skills_from_array(data.as_array().unwrap());
        assert_eq!(
            entries.iter().map(|item| item.installs).collect::<Vec<_>>(),
            [3, 4]
        );
    }
}
