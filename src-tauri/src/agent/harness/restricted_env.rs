//! 受限执行环境:为 Wiki 内置 Agent 包装任意 [`ExecutionEnv`],把文件访问收敛到
//! 「项目根只读 + 单一允许写入文件」的最小权限面。
//!
//! 策略:
//! - 读(read/read_lines/read_binary/file_info/list_dir/canonical_path/exists/
//!   absolute_path)仅允许 `project_root` 与 `allowed_write_file` 两个根
//!   (后者即使位于项目外的 wiki 目录也允许读自身);
//! - 写/append 仅允许 `allowed_write_file` 本身(其必要父目录由内层
//!   `write_file` 自动创建);rename 的源与目标都必须是 `allowed_write_file`;
//!   remove 仅允许 `allowed_write_file`;create_dir 仅允许其必要父目录链
//!   (即 `allowed_write_file` 的严格祖先);
//! - shell exec 一律拒绝;临时目录/临时文件创建一律拒绝(系统临时目录在根之外);
//! - `join_path` 不触盘,与内层行为一致仅做字符串拼接,不另设限制。
//!
//! 路径安全:
//! - 先经内层 `absolute_path` 解析(相对/`~`/`..` 词法归一化,与被包装环境
//!   完全一致),再基于归一化结果做检查;
//! - 除词法包含检查外,对目标额外做「最近既存祖先 canonicalize + 词法尾巴」
//!   的规范形检查,拦截项目内符号链接指向根外路径的逃逸(grep/find 等工具
//!   resolve 后会直接遍历,必须在解析入口拦截,故 `absolute_path` 返回前同样
//!   做根与符号链接检查);
//! - Windows 下路径组件比较大小写不敏感,并剥离 canonicalize 产出的
//!   `\\?\`/`\\?\UNC\` verbatim 前缀;其他平台大小写敏感。
//!
//! 检查在调用前完成(非内核级沙箱):不防本进程其他代码的 TOCTOU,只约束
//! 经过该 env 的工具调用面。错误统一返回 `PermissionDenied` 的
//! [`FileError`](crate::agent::harness::types::FileError),shell 拒绝返回
//! `spawn_error` 的 [`ExecutionError`](crate::agent::harness::types::ExecutionError)。

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent::harness::env::TokioEnv;
use crate::agent::harness::types::{
    err, ok, CreateDirOptions, CreateTempFileOptions, ExecOutcome, ExecutionEnv, ExecutionError,
    ExecutionErrorCode, FileContent, FileError, FileErrorCode, FileInfo, FileSystem,
    ReadTextLinesOptions, RemoveOptions, Result, Shell, ShellExecOptions, SimpleError,
};
use crate::agent::types::AbortSignal;

// ---------------------------------------------------------------------------
// 路径辅助
// ---------------------------------------------------------------------------

/// 词法归一化:与 `env.rs` 的 `normalize_absolute` 同语义(切分 `/` 与 `\`,
/// 展开 `.`/`..`,不触盘),此处独立实现以保持本模块自包含。
fn normalize_absolute(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let mut prefix: Option<String> = None;
    let mut body_start = 0usize;
    if cfg!(windows) && text.len() >= 2 && text.as_bytes()[1] == b':' {
        prefix = Some(text[..2].to_string());
        body_start = 2;
    }

    let mut stack: Vec<String> = prefix.iter().cloned().collect();
    for component in text[body_start..].split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => {
                if stack.len() > prefix.as_ref().map_or(0, |_| 1) {
                    stack.pop();
                }
            }
            other => stack.push(other.to_string()),
        }
    }

    let mut result = PathBuf::new();
    if let Some(prefix) = &prefix {
        // 显式补根分隔符:裸 `C:` 上 push 会退化成盘符相对路径 `C:Users`
        // (env.rs 同款词法归一化在 Windows 上的已知怪癖,盘符相对路径经
        // 「该盘当前目录」解析,恰好为根时才等价)。这里统一归一为 `C:\…`,
        // 同时修复内层 absolute_path 对绝对输入产出的盘符相对形态。
        result.push(format!("{prefix}\\"));
        for part in &stack[1..] {
            result.push(part);
        }
    } else {
        result.push("/");
        for part in stack {
            result.push(part);
        }
    }
    result
}

/// 剥离 Windows canonicalize 产出的 verbatim 前缀(`\\?\C:\x` → `C:\x`,
/// `\\?\UNC\s\x` → `\\s\x`),使规范形与常规绝对路径可比较。
#[cfg(windows)]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

#[cfg(not(windows))]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

/// 路径组件列表;Windows 下统一小写以做大小写不敏感比较。
fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter(|component| !matches!(component, Component::CurDir))
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .map(|component| {
            if cfg!(windows) {
                component.to_lowercase()
            } else {
                component
            }
        })
        .collect()
}

/// `path` 是否位于 `root` 内(含相等)。按组件前缀比较,避免 `/proj` 误含
/// `/proj2` 这类字符串前缀假阳性。
fn is_within(root: &Path, path: &Path) -> bool {
    let root_components = path_components(root);
    let path_components = path_components(path);
    path_components.len() >= root_components.len()
        && root_components
            .iter()
            .zip(&path_components)
            .all(|(root_part, path_part)| root_part == path_part)
}

/// 两个路径是否指向同一位置(组件完全一致,忽略大小写差异仅在 Windows)。
fn same_path(a: &Path, b: &Path) -> bool {
    path_components(a) == path_components(b)
}

/// `ancestor` 是否为 `path` 的严格祖先(真前缀,非相等)。
fn is_strict_ancestor(ancestor: &Path, path: &Path) -> bool {
    path_components(ancestor).len() < path_components(path).len() && is_within(ancestor, path)
}

/// 规范形:从目标向上找到最近一个 canonicalize 成功的既存祖先,拼回其下的
/// 词法尾巴(尾巴组件均不存在,不可能含符号链接)。用于拦截「项目内 symlink
/// 指向根外」的逃逸,同时兼容根路径自身经符号链接(macOS `/tmp` 等)的场景。
async fn canonical_form(path: &Path) -> std::io::Result<PathBuf> {
    let total = path.components().count();
    for (index, ancestor) in path.ancestors().enumerate() {
        if let Ok(anchor) = tokio::fs::canonicalize(ancestor).await {
            let mut result = strip_verbatim_prefix(anchor);
            let skip = total.saturating_sub(index);
            for component in path.components().skip(skip) {
                result.push(component);
            }
            return Ok(result);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no existing ancestor to canonicalize",
    ))
}

/// [`canonical_form`] 的同步版(构造期对根算一次)。
fn canonical_form_sync(path: &Path) -> Option<PathBuf> {
    let total = path.components().count();
    for (index, ancestor) in path.ancestors().enumerate() {
        if let Ok(anchor) = std::fs::canonicalize(ancestor) {
            let mut result = strip_verbatim_prefix(anchor);
            let skip = total.saturating_sub(index);
            for component in path.components().skip(skip) {
                result.push(component);
            }
            return Some(result);
        }
    }
    None
}

fn deny(action: &str, resolved: &Path) -> FileError {
    FileError::new(
        FileErrorCode::PermissionDenied,
        format!(
            "restricted environment: {action} is not allowed outside the project root and the permitted write target: {}",
            resolved.to_string_lossy()
        ),
    )
    .with_path(resolved.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// 根描述与受限环境
// ---------------------------------------------------------------------------

/// 受检根的两种形态:词法归一化路径 + 构造期计算的规范形
/// (最近既存祖先 canonicalize + 词法尾巴)。
#[derive(Debug, Clone)]
struct RootSpec {
    /// 词法归一化后的绝对路径(平台分隔符)。
    lexical: PathBuf,
    /// 规范形;根完全不存在且无既存祖先可 canonicalize 时为 None(仅词法检查)。
    canonical: Option<PathBuf>,
}

impl RootSpec {
    fn new(path: &Path) -> Result<Self, SimpleError> {
        if !path.is_absolute() {
            return Err(SimpleError::new(format!(
                "restricted environment: root path must be absolute: {}",
                path.to_string_lossy()
            )));
        }
        let lexical = normalize_absolute(path);
        Ok(Self {
            canonical: canonical_form_sync(&lexical),
            lexical,
        })
    }
}

/// 受限执行环境:包装内层 env,按模块文档的策略收敛读写面。
pub struct RestrictedEnv {
    inner: Arc<dyn ExecutionEnv>,
    project_root: RootSpec,
    allowed_write_file: RootSpec,
}

impl RestrictedEnv {
    /// 包装既有 env(其 cwd 决定相对路径基准,通常应设为 `project_root`)。
    pub fn new(
        inner: Arc<dyn ExecutionEnv>,
        project_root: impl AsRef<Path>,
        allowed_write_file: impl AsRef<Path>,
    ) -> Result<Self, SimpleError> {
        Ok(Self {
            inner,
            project_root: RootSpec::new(project_root.as_ref())?,
            allowed_write_file: RootSpec::new(allowed_write_file.as_ref())?,
        })
    }

    /// 便捷构造:以内层 `TokioEnv`(cwd = 项目根)包装出 Wiki 内置 Agent 用的
    /// 受限环境。
    pub fn for_wiki_agent(
        project_root: impl AsRef<Path>,
        allowed_write_file: impl AsRef<Path>,
    ) -> Result<Arc<dyn ExecutionEnv>, SimpleError> {
        let project_root = project_root.as_ref();
        if !project_root.is_absolute() {
            return Err(SimpleError::new(format!(
                "restricted environment: root path must be absolute: {}",
                project_root.to_string_lossy()
            )));
        }
        let normalized_root = normalize_absolute(project_root);
        let inner = Arc::new(TokioEnv::new(normalized_root.clone()));
        Ok(Arc::new(Self::new(inner, normalized_root, allowed_write_file)?))
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root.lexical
    }

    pub fn allowed_write_file(&self) -> &Path {
        &self.allowed_write_file.lexical
    }

    /// 经内层解析(与被包装环境完全一致的相对/`~`/`..` 语义),再做一次词法
    /// 归一化兜底。内层 `absolute_path` 不触盘、不失败。
    async fn resolve(&self, path: &str) -> Result<PathBuf, FileError> {
        let resolved = self.inner.absolute_path(path.to_string(), None).await?;
        Ok(normalize_absolute(Path::new(&resolved)))
    }

    /// 读检查:目标须位于 project_root 或 allowed_write_file 之内
    /// (词法或规范形任一命中)。
    async fn ensure_readable(&self, resolved: &Path) -> Result<(), FileError> {
        if is_within(&self.project_root.lexical, resolved)
            || is_within(&self.allowed_write_file.lexical, resolved)
        {
            // 词法命中仍需符号链接检查:项目内 symlink 可能指向根外。
        } else {
            let lexical_pass = false;
            if !lexical_pass {
                // 走规范形比较(见下)。
            }
        }
        if let Ok(canonical) = canonical_form(resolved).await {
            if let Some(root) = &self.project_root.canonical {
                if is_within(root, &canonical) {
                    return Ok(());
                }
            }
            if let Some(allowed) = &self.allowed_write_file.canonical {
                if is_within(allowed, &canonical) {
                    return Ok(());
                }
            }
        }
        Err(deny("read", resolved))
    }

    /// 写目标检查:必须就是 allowed_write_file(词法或规范形相等)。
    async fn ensure_writable_target(&self, resolved: &Path) -> Result<(), FileError> {
        if same_path(&self.allowed_write_file.lexical, resolved) {
            return Ok(());
        }
        if let (Some(allowed), Ok(canonical)) = (
            &self.allowed_write_file.canonical,
            canonical_form(resolved).await,
        ) {
            if same_path(allowed, &canonical) {
                return Ok(());
            }
        }
        Err(deny("write", resolved))
    }

    /// 目录变更检查:仅允许 allowed_write_file 的必要父目录链(严格祖先)。
    async fn ensure_write_parent(&self, resolved: &Path) -> Result<(), FileError> {
        if is_strict_ancestor(resolved, &self.allowed_write_file.lexical) {
            return Ok(());
        }
        if let (Some(allowed), Ok(canonical)) = (
            &self.allowed_write_file.canonical,
            canonical_form(resolved).await,
        ) {
            if is_strict_ancestor(&canonical, allowed) {
                return Ok(());
            }
        }
        Err(deny("directory mutation", resolved))
    }
}

impl FileSystem for RestrictedEnv {
    fn cwd(&self) -> &str {
        self.inner.cwd()
    }

    fn absolute_path<'a>(
        &'a self,
        path: String,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            // grep/find 等 resolve 后直接遍历返回值,必须在入口拦截根外与
            // symlink 逃逸;返回值保持词法形态(不解析符号链接,契约不变)。
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            ok(resolved.to_string_lossy().to_string())
        })
    }

    fn join_path<'a>(
        &'a self,
        parts: Vec<String>,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            // 不触盘的纯拼接,与内层一致;后续方法使用结果时各自受限。
            self.inner.join_path(parts, None).await
        })
    }

    fn read_text_file<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            self.inner
                .read_text_file(resolved.to_string_lossy().to_string(), abort_signal)
                .await
        })
    }

    fn read_text_lines<'a>(
        &'a self,
        path: String,
        options: ReadTextLinesOptions,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            self.inner
                .read_text_lines(resolved.to_string_lossy().to_string(), options)
                .await
        })
    }

    fn read_binary_file<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            self.inner
                .read_binary_file(resolved.to_string_lossy().to_string(), abort_signal)
                .await
        })
    }

    fn write_file<'a>(
        &'a self,
        path: String,
        content: FileContent,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_writable_target(&resolved).await?;
            self.inner
                .write_file(resolved.to_string_lossy().to_string(), content, abort_signal)
                .await
        })
    }

    fn append_file<'a>(
        &'a self,
        path: String,
        content: FileContent,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_writable_target(&resolved).await?;
            self.inner
                .append_file(resolved.to_string_lossy().to_string(), content)
                .await
        })
    }

    fn rename_file<'a>(
        &'a self,
        source_path: String,
        destination_path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let source = self.resolve(&source_path).await?;
            let destination = self.resolve(&destination_path).await?;
            // 仅允许把 allowed_write_file 重命名为其自身(防御性收紧:任何
            // 换名都会把唯一可写目标挪出受控范围)。
            self.ensure_writable_target(&source).await?;
            self.ensure_writable_target(&destination).await?;
            self.inner
                .rename_file(
                    source.to_string_lossy().to_string(),
                    destination.to_string_lossy().to_string(),
                    abort_signal,
                )
                .await
        })
    }

    fn file_info<'a>(&'a self, path: String) -> BoxFuture<'a, Result<FileInfo, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            self.inner
                .file_info(resolved.to_string_lossy().to_string())
                .await
        })
    }

    fn list_dir<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            self.inner
                .list_dir(resolved.to_string_lossy().to_string(), abort_signal)
                .await
        })
    }

    fn canonical_path<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            self.inner
                .canonical_path(resolved.to_string_lossy().to_string(), abort_signal)
                .await
        })
    }

    fn exists<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<bool, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_readable(&resolved).await?;
            self.inner
                .exists(resolved.to_string_lossy().to_string(), abort_signal)
                .await
        })
    }

    fn create_dir<'a>(
        &'a self,
        path: String,
        options: CreateDirOptions,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_write_parent(&resolved).await?;
            self.inner
                .create_dir(resolved.to_string_lossy().to_string(), options)
                .await
        })
    }

    fn remove<'a>(&'a self, path: String, options: RemoveOptions) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path).await?;
            self.ensure_writable_target(&resolved).await?;
            self.inner
                .remove(resolved.to_string_lossy().to_string(), options)
                .await
        })
    }

    fn create_temp_dir<'a>(
        &'a self,
        _prefix: Option<String>,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move { err(deny("temporary directory creation", &std::env::temp_dir())) })
    }

    fn create_temp_file<'a>(
        &'a self,
        _options: CreateTempFileOptions,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move { err(deny("temporary file creation", &std::env::temp_dir())) })
    }

    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()> {
        // Arc<dyn ExecutionEnv> 同时可见 FileSystem/Shell 两套 cleanup,需显式消歧。
        FileSystem::cleanup(self.inner.as_ref())
    }
}

impl Shell for RestrictedEnv {
    fn exec<'a>(
        &'a self,
        _command: String,
        _options: ShellExecOptions,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecutionError>> {
        Box::pin(async move {
            err(ExecutionError::new(
                ExecutionErrorCode::SpawnError,
                "restricted environment: shell execution is disabled (read/write only)",
            ))
        })
    }

    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()> {
        FileSystem::cleanup(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn unique_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "repomeow-restricted-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// 测试目录树:
    /// base/
    ///   project/          ← project_root
    ///     src/main.rs
    ///     readme.md
    ///   wiki/pages/page.md ← allowed_write_file(位于项目外)
    ///   outside.txt        ← 两个根之外
    struct TestTree {
        base: PathBuf,
    }

    impl TestTree {
        fn new() -> Self {
            let base = unique_base("tree");
            fs::create_dir_all(base.join("project/src")).unwrap();
            fs::create_dir_all(base.join("wiki/pages")).unwrap();
            fs::write(base.join("project/src/main.rs"), "fn main() {}\n").unwrap();
            fs::write(base.join("project/readme.md"), "# readme\n").unwrap();
            fs::write(base.join("outside.txt"), "outside\n").unwrap();
            fs::write(base.join("wiki/pages/page.md"), "# Page\n").unwrap();
            Self { base }
        }

        fn project_root(&self) -> PathBuf {
            self.base.join("project")
        }

        fn allowed(&self) -> PathBuf {
            self.base.join("wiki/pages/page.md")
        }

        fn outside_file(&self) -> PathBuf {
            self.base.join("outside.txt")
        }

        fn env(&self) -> Arc<dyn ExecutionEnv> {
            RestrictedEnv::for_wiki_agent(self.project_root(), self.allowed()).unwrap()
        }

        fn env_with_allowed(&self, allowed: &Path) -> Arc<dyn ExecutionEnv> {
            RestrictedEnv::for_wiki_agent(self.project_root(), allowed).unwrap()
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base).ok();
        }
    }

    fn assert_denied(result: Result<(), FileError>) {
        let error = result.unwrap_err();
        assert_eq!(
            error.code,
            FileErrorCode::PermissionDenied,
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn reads_inside_project_root_allowed() {
        let tree = TestTree::new();
        let env = tree.env();

        let text = env.read_text_file("src/main.rs".to_string(), None).await.unwrap();
        assert_eq!(text, "fn main() {}\n");

        let absolute = env
            .read_text_file(tree.project_root().join("src/main.rs").to_string_lossy().to_string(), None)
            .await
            .unwrap();
        assert_eq!(absolute, "fn main() {}\n");

        let lines = env
            .read_text_lines(
                tree.project_root().join("readme.md").to_string_lossy().to_string(),
                ReadTextLinesOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(lines, vec!["# readme"]);

        assert!(env.exists("src/main.rs".to_string(), None).await.unwrap());
        let info = env.file_info("src/main.rs".to_string()).await.unwrap();
        assert_eq!(info.kind, crate::agent::harness::types::FileKind::File);
    }

    #[tokio::test]
    async fn reads_outside_roots_denied() {
        let tree = TestTree::new();
        let env = tree.env();
        let outside = tree.outside_file().to_string_lossy().to_string();

        // 读文件、exists、file_info、canonical_path 全部拒绝。
        assert_denied(env.read_text_file(outside.clone(), None).await.map(|_| ()));
        assert_denied(env.exists(outside.clone(), None).await.map(|_| ()));
        assert_denied(env.file_info(outside.clone()).await.map(|_| ()));
        assert_denied(env.canonical_path(outside.clone(), None).await.map(|_| ()));
        // absolute_path 是 grep/find 的解析入口,同样必须拒绝。
        let absolute = env.absolute_path(outside.clone(), None).await;
        assert_eq!(absolute.unwrap_err().code, FileErrorCode::PermissionDenied);
    }

    #[tokio::test]
    async fn dotdot_escape_denied() {
        let tree = TestTree::new();
        let env = tree.env();

        assert_denied(env.read_text_file("../outside.txt".to_string(), None).await.map(|_| ()));
        assert_denied(
            env.read_text_file("src/../../outside.txt".to_string(), None)
                .await
                .map(|_| ()),
        );
        assert_denied(env.absolute_path("../outside.txt".to_string(), None).await.map(|_| ()));

        // `..` 不越界时正常解析。
        let resolved = env.absolute_path("src/../src/main.rs".to_string(), None).await.unwrap();
        assert!(Path::new(&resolved).ends_with("src/main.rs"), "{resolved}");
    }

    #[tokio::test]
    async fn sibling_directory_boundary_denied() {
        let tree = TestTree::new();
        let env = tree.env();
        // 字符串前缀相近但组件不同:project2 不在 project 内。
        let sibling = format!("{}2/secret.txt", tree.project_root().to_string_lossy());
        assert_denied(env.read_text_file(sibling, None).await.map(|_| ()));
    }

    #[tokio::test]
    async fn allowed_write_file_readable_outside_project() {
        let tree = TestTree::new();
        let env = tree.env();

        let allowed = tree.allowed().to_string_lossy().to_string();
        let text = env.read_text_file(allowed.clone(), None).await.unwrap();
        assert_eq!(text, "# Page\n");

        // 相对项目根的 `..` 路径命中 allowed_write_file,同样允许。
        let text = env
            .read_text_file("../wiki/pages/page.md".to_string(), None)
            .await
            .unwrap();
        assert_eq!(text, "# Page\n");

        // 但 allowed 文件所在目录(项目外)不允许列目录/读同级文件。
        let sibling = tree.base.join("wiki/pages/other.md").to_string_lossy().to_string();
        assert_denied(env.read_text_file(sibling, None).await.map(|_| ()));
        let wiki_dir = tree.base.join("wiki/pages").to_string_lossy().to_string();
        assert_denied(env.list_dir(wiki_dir, None).await.map(|_| ()));
    }

    #[tokio::test]
    async fn list_dir_confined_to_project_root() {
        let tree = TestTree::new();
        let env = tree.env();

        let entries = env.list_dir(".".to_string(), None).await.unwrap();
        let names: Vec<String> = entries.iter().map(|info| info.name.clone()).collect();
        assert!(names.contains(&"src".to_string()), "{names:?}");
        assert!(names.contains(&"readme.md".to_string()), "{names:?}");

        let cwd = env.cwd();
        assert!(Path::new(cwd).ends_with("project"), "{cwd}");
    }

    #[tokio::test]
    async fn write_and_append_only_allowed_file() {
        let tree = TestTree::new();
        // 深层不存在父目录的 allowed 目标:写时自动建必要父目录。
        let allowed = tree.base.join("wiki/pages/deep/new-page.md");
        let env = tree.env_with_allowed(&allowed);

        let allowed_text = allowed.to_string_lossy().to_string();
        env.write_file(allowed_text.clone(), FileContent::Text("body".to_string()), None)
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&allowed).unwrap(), "body");
        env.append_file(allowed_text.clone(), FileContent::Text("+more".to_string()))
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&allowed).unwrap(), "body+more");

        // 项目内其他文件、wiki 同级文件:写一律拒绝。
        assert_denied(
            env.write_file(
                tree.project_root().join("readme.md").to_string_lossy().to_string(),
                FileContent::Text("hack".to_string()),
                None,
            )
            .await,
        );
        assert_denied(
            env.write_file("relative-new-file.txt".to_string(), FileContent::Text("x".to_string()), None)
                .await,
        );
        let wiki_sibling = tree.base.join("wiki/pages/other.md").to_string_lossy().to_string();
        assert_denied(env.write_file(wiki_sibling, FileContent::Text("x".to_string()), None).await);
    }

    #[tokio::test]
    async fn remove_policy_only_allowed_file() {
        let tree = TestTree::new();
        let env = tree.env();

        // 其他路径(即使在项目内)不允许删除。
        assert_denied(
            env.remove(
                tree.project_root().join("readme.md").to_string_lossy().to_string(),
                RemoveOptions::default(),
            )
            .await,
        );
        assert!(tree.project_root().join("readme.md").exists());

        // allowed 文件本身可删后重写。
        let allowed = tree.allowed().to_string_lossy().to_string();
        env.remove(allowed.clone(), RemoveOptions::default()).await.unwrap();
        assert!(!tree.allowed().exists());
        env.write_file(allowed, FileContent::Text("# Page\n".to_string()), None)
            .await
            .unwrap();
        assert!(tree.allowed().exists());
    }

    #[tokio::test]
    async fn rename_policy_source_and_target_must_be_allowed() {
        let tree = TestTree::new();
        let env = tree.env();
        let allowed = tree.allowed().to_string_lossy().to_string();

        // 源不是 allowed:拒绝。
        assert_denied(
            env.rename_file(
                tree.project_root().join("readme.md").to_string_lossy().to_string(),
                allowed.clone(),
                None,
            )
            .await,
        );
        // 目标不是 allowed:拒绝(防止把唯一可写目标挪出受控范围)。
        assert_denied(
            env.rename_file(
                allowed.clone(),
                tree.base.join("wiki/pages/renamed.md").to_string_lossy().to_string(),
                None,
            )
            .await,
        );
        // 源与目标都是 allowed(自身):允许。
        env.rename_file(allowed.clone(), allowed, None).await.unwrap();
    }

    #[tokio::test]
    async fn create_dir_policy_necessary_parents_only() {
        let tree = TestTree::new();
        // allowed 的父目录链允许创建。
        let allowed = tree.base.join("wiki/pages/extra/new.md");
        let env = tree.env_with_allowed(&allowed);
        let parent = tree.base.join("wiki/pages/extra").to_string_lossy().to_string();
        env.create_dir(parent, CreateDirOptions::default()).await.unwrap();
        assert!(tree.base.join("wiki/pages/extra").is_dir());

        // 项目内无关目录:拒绝。
        let env = tree.env();
        let build_dir = tree.project_root().join("build").to_string_lossy().to_string();
        assert_denied(env.create_dir(build_dir, CreateDirOptions::default()).await);
        assert_denied(env.create_dir("sub/dir".to_string(), CreateDirOptions::default()).await);
        // 在 allowed 文件路径本身建目录会遮蔽写目标:拒绝。
        assert_denied(env.create_dir(tree.allowed().to_string_lossy().to_string(), CreateDirOptions::default()).await);
    }

    #[tokio::test]
    async fn shell_exec_always_denied() {
        let tree = TestTree::new();
        let env = tree.env();
        let error = env
            .exec("echo hello".to_string(), ShellExecOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.code, ExecutionErrorCode::SpawnError, "{error}");
        assert!(error.message.contains("restricted environment"), "{error}");
    }

    #[tokio::test]
    async fn temp_creation_denied() {
        let tree = TestTree::new();
        let env = tree.env();
        assert_denied(env.create_temp_dir(None, None).await.map(|_| ()));
        assert_denied(env.create_temp_file(CreateTempFileOptions::default()).await.map(|_| ()));
    }

    #[tokio::test]
    async fn allowed_file_inside_project_works() {
        let tree = TestTree::new();
        let allowed = tree.project_root().join("notes.md");
        let env = tree.env_with_allowed(&allowed);

        env.write_file("notes.md".to_string(), FileContent::Text("note".to_string()), None)
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&allowed).unwrap(), "note");
        // 同项目内其他文件仍然只读。
        assert!(env.read_text_file("readme.md".to_string(), None).await.is_ok());
        assert_denied(
            env.write_file("readme.md".to_string(), FileContent::Text("x".to_string()), None)
                .await,
        );
    }

    #[tokio::test]
    async fn constructor_rejects_relative_roots() {
        let tree = TestTree::new();
        let inner: Arc<dyn ExecutionEnv> = Arc::new(TokioEnv::new(tree.project_root()));
        assert!(RestrictedEnv::new(inner.clone(), Path::new("relative/root"), tree.allowed()).is_err());
        assert!(RestrictedEnv::for_wiki_agent("relative/root", tree.allowed()).is_err());
    }

    #[tokio::test]
    async fn wraps_existing_env() {
        // 显式包装既有 TokioEnv(cwd = 项目根)的构造路径。
        let tree = TestTree::new();
        let inner: Arc<dyn ExecutionEnv> = Arc::new(TokioEnv::new(tree.project_root()));
        let env = Arc::new(
            RestrictedEnv::new(inner, tree.project_root(), tree.allowed()).unwrap(),
        ) as Arc<dyn ExecutionEnv>;
        let text = env.read_text_file("src/main.rs".to_string(), None).await.unwrap();
        assert_eq!(text, "fn main() {}\n");
        assert_denied(env.read_text_file(tree.outside_file().to_string_lossy().to_string(), None).await.map(|_| ()));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_paths_match_case_insensitively() {
        let tree = TestTree::new();
        let env = tree.env();

        // 项目内读:大小写不同仍应命中项目根。
        let text = env
            .read_text_file("SRC/MAIN.RS".to_string(), None)
            .await
            .unwrap();
        assert_eq!(text, "fn main() {}\n");

        // allowed 文件大小写不同:写应允许(NTFS 大小写不敏感)。
        let upper = tree.allowed().to_string_lossy().to_uppercase();
        env.write_file(upper, FileContent::Text("# Page\n".to_string()), None)
            .await
            .unwrap();

        // 根外路径即使大小写变化仍被拒绝。
        let outside_upper = tree.outside_file().to_string_lossy().to_uppercase();
        assert_denied(env.read_text_file(outside_upper, None).await.map(|_| ()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_paths_stay_case_sensitive() {
        let tree = TestTree::new();
        let env = tree.env();
        assert_denied(env.read_text_file("README.MD".to_string(), None).await.map(|_| ()));
        assert!(env.read_text_file("readme.md".to_string(), None).await.is_ok());
    }

    mod symlinks {
        use super::*;

        /// 平台 symlink 创建:unix 原生;Windows 需要管理员/开发者模式,
        /// 无权限时返回 Err,调用方跳过用例(语义一致,只是能力差异)。
        fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, link)
            }
            #[cfg(windows)]
            {
                if target.is_dir() {
                    std::os::windows::fs::symlink_dir(target, link)
                } else {
                    std::os::windows::fs::symlink_file(target, link)
                }
            }
        }

        fn symlink_or_skip(target: &Path, link: &Path) -> bool {
            if create_symlink(target, link).is_err() {
                eprintln!("skipped: symlink creation not permitted on this host");
                return false;
            }
            true
        }

        #[tokio::test]
        async fn symlink_dir_escape_denied() {
            let tree = TestTree::new();
            let outside_dir = unique_base("outside-dir");
            fs::create_dir_all(&outside_dir).unwrap();
            fs::write(outside_dir.join("secret.txt"), "secret\n").unwrap();
            if !symlink_or_skip(&outside_dir, &tree.project_root().join("evil-link")) {
                return;
            }

            let env = tree.env();
            // 目录 symlink 本身与其子路径:词法在项目内,canonical 在外 → 拒绝。
            assert_denied(env.read_text_file("evil-link".to_string(), None).await.map(|_| ()));
            assert_denied(
                env.read_text_file("evil-link/secret.txt".to_string(), None)
                    .await
                    .map(|_| ()),
            );
            // absolute_path 是遍历入口,同样拒绝(candidate 是项目内 symlink 时防逃逸)。
            assert_denied(env.absolute_path("evil-link/secret.txt".to_string(), None).await.map(|_| ()));
            assert_denied(env.list_dir("evil-link".to_string(), None).await.map(|_| ()));

            fs::remove_dir_all(&outside_dir).ok();
        }

        #[tokio::test]
        async fn symlink_file_escape_denied() {
            let tree = TestTree::new();
            if !symlink_or_skip(&tree.outside_file(), &tree.project_root().join("leak.md")) {
                return;
            }

            let env = tree.env();
            assert_denied(env.read_text_file("leak.md".to_string(), None).await.map(|_| ()));
            assert_denied(env.canonical_path("leak.md".to_string(), None).await.map(|_| ()));
        }

        #[tokio::test]
        async fn symlink_pointing_inside_root_allowed() {
            let tree = TestTree::new();
            fs::create_dir_all(tree.project_root().join("sub")).unwrap();
            fs::write(tree.project_root().join("sub/data.txt"), "data\n").unwrap();
            if !symlink_or_skip(
                &tree.project_root().join("sub"),
                &tree.project_root().join("alias"),
            ) {
                return;
            }

            let env = tree.env();
            let text = env
                .read_text_file("alias/data.txt".to_string(), None)
                .await
                .unwrap();
            assert_eq!(text, "data\n");
        }

        #[tokio::test]
        async fn allowed_file_reachable_via_symlink_allowed() {
            let tree = TestTree::new();
            // 项目外 wiki 目录中的 symlink 指向 allowed 文件:canonical 等价 → 允许。
            if !symlink_or_skip(&tree.allowed(), &tree.base.join("wiki/page-link.md")) {
                return;
            }

            let env = tree.env();
            let link = tree.base.join("wiki/page-link.md").to_string_lossy().to_string();
            let text = env.read_text_file(link, None).await.unwrap();
            assert_eq!(text, "# Page\n");
        }

        #[tokio::test]
        async fn dangling_symlink_reports_not_found() {
            let tree = TestTree::new();
            if !symlink_or_skip(&tree.base.join("nowhere.txt"), &tree.project_root().join("dangling.md")) {
                return;
            }

            let env = tree.env();
            // 词法/规范形均在项目内 → 放行给内层,内层报 NotFound(非 panic)。
            let error = env
                .read_text_file("dangling.md".to_string(), None)
                .await
                .unwrap_err();
            assert_eq!(error.code, crate::agent::harness::types::FileErrorCode::NotFound, "{error}");
        }
    }
}
