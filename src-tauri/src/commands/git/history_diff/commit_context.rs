use super::{super::*, truncate_chars};

/// 送入 AI 的 diff 长度上限(超出截断,避免 token 爆炸)
const DIFF_MAX_CHARS: usize = 30_000;
/// 单个未跟踪文件内容上限(字符)
const UNTRACKED_FILE_MAX_CHARS: usize = 4_000;
/// 全部未跟踪文件内容的总预算(字符)
const UNTRACKED_TOTAL_MAX_CHARS: usize = 12_000;
/// 二进制嗅探的前缀长度(含 NUL 即视为二进制)
const BINARY_SNIFF_BYTES: usize = 8_000;

/// diff 噪声文件:内容对撰写提交信息无意义,排除正文以节省 token 预算。
/// 这里只列各生态明确由工具生成的锁定/校验文件，避免用 `*.lock`、`dist/**` 等宽泛规则误伤源码。
/// stat 仍保留这些文件，因“依赖锁文件变了”本身对提交信息有价值。
const DIFF_EXCLUDES: &[&str] = &[
    // JavaScript / Node.js
    "pnpm-lock.yaml",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    // Rust / Go
    "Cargo.lock",
    "go.sum",
    "go.work.sum",
    // Python
    "uv.lock",
    "poetry.lock",
    "Pipfile.lock",
    "pdm.lock",
    "pixi.lock",
    // JVM / .NET
    "gradle.lockfile",
    "packages.lock.json",
    "paket.lock",
    // PHP / Ruby / Elixir
    "composer.lock",
    "Gemfile.lock",
    "mix.lock",
    // Apple / Dart / Nix / Terraform / 其他包管理器
    "Package.resolved",
    "Podfile.lock",
    "Cartfile.resolved",
    "pubspec.lock",
    "flake.lock",
    ".terraform.lock.hcl",
    "Chart.lock",
    "conan.lock",
    "deno.lock",
    "Gopkg.lock",
    "glide.lock",
];

/// 明确由构建工具生成、正文通常不可读的产物后缀。仅匹配文件名，目录名不参与判断。
const DIFF_EXCLUDE_SUFFIXES: &[&str] = &[
    ".min.js",
    ".min.mjs",
    ".min.cjs",
    ".min.css",
    ".map",
    ".map.gz",
    ".map.br",
    ".lockfile",
];

fn diff_file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// 判断是否只保留 stat、不把正文交给 sem/LLM。匹配精确 basename 或保守的生成物后缀；
/// 普通 `.lock`、源码目录下的 `dist`、快照与声明文件仍保留正文。
fn is_diff_excluded(path: &str) -> bool {
    let file_name = diff_file_name(path);
    DIFF_EXCLUDES.contains(&file_name)
        || DIFF_EXCLUDE_SUFFIXES
            .iter()
            .any(|suffix| file_name.ends_with(suffix))
}

/// 读取未跟踪新文件的文本内容;非常规文件/二进制/读失败返回 None(由调用方回退到仅列文件名)
fn read_untracked_file(repo: &str, rel: &str) -> Option<GitUntrackedFile> {
    let full = Path::new(repo).join(rel);
    let meta = std::fs::metadata(&full).ok()?;
    if !meta.is_file() {
        return None;
    }
    // 按字节多读一段用于二进制嗅探;char 截断在解码后做(UTF-8 最多 4 字节/字符,预算留足)
    let max_bytes = (UNTRACKED_FILE_MAX_CHARS * 4 + BINARY_SNIFF_BYTES) as u64;
    let mut buf = Vec::new();
    std::fs::File::open(&full)
        .ok()?
        .take(max_bytes)
        .read_to_end(&mut buf)
        .ok()?;
    if buf[..buf.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    let (content, char_truncated) = truncate_chars(&text, UNTRACKED_FILE_MAX_CHARS);
    Some(GitUntrackedFile {
        path: rel.to_string(),
        content,
        truncated: char_truncated || meta.len() > buf.len() as u64,
    })
}

/// 收集 AI 生成提交信息所需的变更上下文:
/// 覆盖已暂存 + 已跟踪未暂存修改(与 git_commit 语义一致,相对 HEAD);
/// 仓库尚无提交(无 HEAD)时回退到暂存区 diff;
/// diff 排除锁文件/min/map 等噪声文件(stat 保留);
/// 未跟踪清单剔除嵌套 git 仓库目录(子仓库是独立项目,不算本仓库内容),
/// 其中可读的文本文件附带内容(预算受限,二进制跳过)
pub(crate) async fn git_commit_context(path: String) -> AppResult<GitCommitContext> {
    run_blocking(move || commit_context_blocking(&path)).await
}

pub(crate) fn commit_context_blocking(path: &str) -> AppResult<GitCommitContext> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };

    // diff:相对 HEAD(等价 git diff HEAD,覆盖已暂存+已跟踪未暂存修改,与 git_commit 语义一致);
    // 仓库尚无提交(无 HEAD)时回退到暂存区 diff(相对空树,等价 git diff --cached)
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = match &head_tree {
        Some(tree) => repo.diff_tree_to_workdir_with_index(Some(tree), None),
        None => {
            let index = repo.index().map_err(git_err)?;
            repo.diff_tree_to_index(None, Some(&index), None)
        }
    }
    .map_err(git_err)?;

    // stat 全量保留(含锁文件等噪声文件);
    // 空 diff 时 libgit2 也会输出 "0 files changed..." 摘要行,与 git --stat 对齐为空串
    let stat = if diff.deltas().len() == 0 {
        String::new()
    } else {
        diff.stats()
            .and_then(|s| {
                s.to_buf(
                    git2::DiffStatsFormat::FULL | git2::DiffStatsFormat::INCLUDE_SUMMARY,
                    80,
                )
            })
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .unwrap_or_default()
    };

    // diff 文本排除锁文件/min/map 等噪声文件(节省 token 预算)
    let mut diff_text = String::new();
    for (idx, delta) in diff.deltas().enumerate() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_diff_excluded(&path) {
            continue;
        }
        if let Ok(Some(mut patch)) = Patch::from_diff(&diff, idx) {
            if let Ok(buf) = patch.to_buf() {
                diff_text.push_str(&String::from_utf8_lossy(&buf));
            }
        }
    }
    let (diff_text, truncated) = truncate_chars(&diff_text, DIFF_MAX_CHARS);

    // 未跟踪清单(等价 ls-files --others --exclude-standard:递归目录、不含忽略文件),
    // 剔除嵌套 git 仓库目录(子仓库是独立项目,不算本仓库内容)
    let mut sopts = StatusOptions::new();
    sopts
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut sopts)).map_err(git_err)?;
    let workdir = repo
        .workdir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    // 缓存本次扫描中已确认的嵌套仓库
    let mut nested_cache: HashSet<String> = HashSet::new();
    let untracked: Vec<String> = statuses
        .iter()
        .filter(|e| e.status().contains(Git2Status::WT_NEW))
        .map(|e| String::from_utf8_lossy(e.path_bytes()).to_string())
        .filter(|p| !is_nested_repo_cached(&workdir, p, &mut nested_cache))
        .collect();

    let mut untracked_files = Vec::new();
    let mut budget = UNTRACKED_TOTAL_MAX_CHARS;
    for name in &untracked {
        if budget == 0 {
            break;
        }
        if let Some(mut f) = read_untracked_file(&workdir, name) {
            let (content, hit_budget) = truncate_chars(&f.content, budget);
            f.truncated = f.truncated || hit_budget;
            budget -= content.chars().count();
            f.content = content;
            untracked_files.push(f);
        }
    }

    Ok(GitCommitContext {
        stat,
        diff: diff_text,
        truncated,
        untracked,
        untracked_files,
    })
}

/// AI 提交信息使用的单文件上下文。raw_patch 只会在 sem 未覆盖该文件或 sem 失败时进入提示词。
#[derive(Debug, Clone)]
pub(crate) struct AiCommitFileContext {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub raw_patch: String,
    pub binary: bool,
    pub raw_excluded: bool,
}

/// 与本次真实提交范围一致的 AI 上下文；semantic_input 是过滤噪声后的 unified patch，
/// 仅在本地交给 sem，不直接发送给模型。
#[derive(Debug)]
pub(crate) struct AiCommitContext {
    pub stat: String,
    pub semantic_input: String,
    pub semantic_paths: HashSet<String>,
    pub files: Vec<AiCommitFileContext>,
}

pub(crate) async fn ai_commit_context(
    path: String,
    include_untracked: bool,
    paths: Option<Vec<String>>,
) -> AppResult<AiCommitContext> {
    run_blocking(move || ai_commit_context_blocking(&path, include_untracked, paths.as_deref()))
        .await
}

fn binary_patch(path: &str, old_path: Option<&str>, status: &str) -> String {
    let old = old_path.unwrap_or(path);
    match status {
        "A" => format!(
            "diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n"
        ),
        "D" => format!(
            "diff --git a/{path} b/{path}\ndeleted file mode 100644\nBinary files a/{path} and /dev/null differ\n"
        ),
        _ => format!(
            "diff --git a/{old} b/{path}\nBinary files a/{old} and b/{path} differ\n"
        ),
    }
}
pub(crate) fn ai_commit_context_blocking(
    path: &str,
    include_untracked: bool,
    paths: Option<&[String]>,
) -> AppResult<AiCommitContext> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    if paths.is_some_and(<[String]>::is_empty) {
        return Err(AppError::coded(ErrorCode::GitPathsRequired, ""));
    }

    let normalized_paths = paths.map(|items| {
        items
            .iter()
            .map(|item| crate::path_util::to_forward_slash_str(item))
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
    });
    let diff = super::worktree_changes::worktree_diff(&repo, |opts| {
        opts.include_untracked(include_untracked)
            .recurse_untracked_dirs(include_untracked)
            .show_untracked_content(include_untracked);
        if let Some(items) = &normalized_paths {
            for item in items {
                opts.pathspec(item);
            }
        }
    })?;
    let workdir = repo
        .workdir()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let mut nested_cache = HashSet::new();
    let mut semantic_patches = Vec::new();
    let mut semantic_paths = HashSet::new();
    let mut files = Vec::new();

    for (idx, delta) in diff.deltas().enumerate() {
        let status = match delta.status() {
            Delta::Added | Delta::Untracked => "A",
            Delta::Copied => "C",
            Delta::Deleted => "D",
            Delta::Modified => "M",
            Delta::Renamed => "R",
            Delta::Typechange => "T",
            _ => continue,
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|value| crate::path_util::to_forward_slash_str(&value.to_string_lossy()))
            .unwrap_or_default();
        if path.is_empty() || is_nested_repo_cached(&workdir, &path, &mut nested_cache) {
            continue;
        }
        let old_path = if matches!(delta.status(), Delta::Renamed | Delta::Copied) {
            delta
                .old_file()
                .path()
                .map(|value| crate::path_util::to_forward_slash_str(&value.to_string_lossy()))
        } else {
            None
        };
        let patch = Patch::from_diff(&diff, idx).ok().flatten();
        let binary = diff
            .get_delta(idx)
            .map(|value| value.flags().is_binary())
            .unwrap_or(false);
        let mut raw_patch = if binary {
            binary_patch(&path, old_path.as_deref(), status)
        } else {
            patch
                .and_then(|mut value| value.to_buf().ok())
                .map(|value| String::from_utf8_lossy(&value).into_owned())
                .unwrap_or_default()
        };
        if raw_patch.trim().is_empty() {
            let old_label = old_path.as_deref().unwrap_or(&path);
            raw_patch = format!("diff --git a/{old_label} b/{path}\n{status} {path}\n");
        }
        let raw_excluded = is_diff_excluded(&path);
        // sem --patch 能从仓库和 patch 恢复实体上下文，无需为每个文件复制完整 before/after。
        // 锁文件、生成物和二进制只保留低成本 stat/fallback 元数据，避免 chunk 边界漂移放大上下文。
        if !binary && !raw_excluded {
            semantic_patches.push(raw_patch.clone());
            semantic_paths.insert(path.clone());
            if let Some(old_path) = &old_path {
                semantic_paths.insert(old_path.clone());
            }
        }
        files.push(AiCommitFileContext {
            path: path.clone(),
            old_path,
            status: status.to_string(),
            raw_patch,
            binary,
            raw_excluded,
        });
    }

    let stat = if files.is_empty() {
        String::new()
    } else {
        files
            .iter()
            .map(|file| {
                let kind = if file.binary {
                    "binary"
                } else {
                    file.status.as_str()
                };
                format!("{} | {kind}", file.path)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let semantic_input = semantic_patches.join("\n");
    Ok(AiCommitContext {
        stat,
        semantic_input,
        semantic_paths,
        files,
    })
}

#[cfg(test)]
mod noise_file_tests {
    use super::*;

    #[test]
    fn excludes_cross_ecosystem_lockfiles_and_generated_assets() {
        for path in [
            "web/pnpm-lock.yaml",
            r"python\uv.lock",
            "ios/Package.resolved",
            "go.sum",
            "infra/.terraform.lock.hcl",
            "gradle/dependency-locks/runtime.lockfile",
            "public/app.min.mjs",
            "public/app.js.map.gz",
        ] {
            assert!(is_diff_excluded(path), "应排除 {path}");
        }
    }

    #[test]
    fn keeps_source_and_ambiguous_lock_named_files() {
        for path in [
            "package.json",
            "Cargo.toml",
            "requirements.txt",
            "src/lock.rs",
            "config/app.lock",
            "docs/yarn.lock.md",
            "dist/source.ts",
            "types/api.generated.d.ts",
            "tests/output.snap",
            "some-package-lock.json",
        ] {
            assert!(!is_diff_excluded(path), "不应排除 {path}");
        }
    }
}
