//! 编辑器真实图标提取:Windows 解析 exe 内嵌图标资源(pelite 重组 ICO + ico 解码),
//! macOS 解析 .app 包的 .icns(plist 读 CFBundleIconFile + icns 解码),统一缩放成
//! 64px PNG 缓存到 <安装目录>/data/icons/<kind>.png,前端经 asset 协议展示。
//! 任何一步失败都返回 None,前端静默回退 lucide 通用图标。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::open;
use crate::db::{self, Db};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::EditorKind;

const ICONS_DIR_NAME: &str = "icons";
/// 图标缓存(目标路径 + mtime)在 settings 表中的 key(JSON: { "<kind>": { target, mtime } })
const ICON_CACHE_SETTING_KEY: &str = "editor_icon_cache";
/// 输出 PNG 边长;源图标更小时不放大,保持原尺寸
const ICON_SIZE: u32 = 64;

/// 全部可提取图标的打开方式(含 explorer / terminal)
const ALL_KINDS: [EditorKind; 15] = [
    EditorKind::Explorer,
    EditorKind::Vscode,
    EditorKind::Cursor,
    EditorKind::Windsurf,
    EditorKind::Trae,
    EditorKind::Vscodium,
    EditorKind::Zed,
    EditorKind::Sublime,
    EditorKind::Idea,
    EditorKind::Webstorm,
    EditorKind::Goland,
    EditorKind::Pycharm,
    EditorKind::Clion,
    EditorKind::Rustrover,
    EditorKind::Terminal,
];

/// 图标缓存条目:图标源文件(Windows exe / macOS icns)与其 mtime,任一变化即重新提取
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IconCacheEntry {
    target: String,
    mtime: u64,
}

type IconCache = HashMap<String, IconCacheEntry>;

/// kind 的前端 id(与 EditorKind 的 serde lowercase 序列化一致)
fn kind_id(kind: EditorKind) -> String {
    format!("{kind:?}").to_ascii_lowercase()
}

fn icons_dir() -> PathBuf {
    crate::runtime_data_root().join(ICONS_DIR_NAME)
}

fn file_mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// 缓存命中条件:目标路径与 mtime 均未变,且 PNG 文件还在
fn cache_hit(entry: Option<&IconCacheEntry>, target: &str, mtime: u64, png_exists: bool) -> bool {
    png_exists && entry.is_some_and(|e| e.target == target && e.mtime == mtime)
}

/// 单个 kind 的完整流程:解析图标源 → 命中缓存直接用,否则提取并写 PNG
fn icon_for_kind(
    kind: EditorKind,
    dir: &Path,
    cache: &mut IconCache,
    dirty: &mut bool,
) -> Option<PathBuf> {
    let id = kind_id(kind);
    let png = dir.join(format!("{id}.png"));
    for target in resolve_icon_candidates(kind) {
        let target_str = target.to_string_lossy().into_owned();
        let Some(mtime) = file_mtime(&target) else {
            continue;
        };
        if cache_hit(cache.get(&id), &target_str, mtime, png.exists()) {
            return Some(png);
        }
        if let Some((w, h, rgba)) = extract_rgba(&target) {
            write_png(&png, w, h, &rgba).ok()?;
            cache.insert(
                id,
                IconCacheEntry {
                    target: target_str,
                    mtime,
                },
            );
            *dirty = true;
            return Some(png);
        }
        // 该候选提取失败(如 AppX 别名 stub),继续下一个候选
    }
    None
}

/// 取全部打开方式的真实图标:kind id → PNG 绝对路径(提取失败为 null)
#[tauri::command]
pub async fn get_editor_icons(db: State<'_, Db>) -> AppResult<HashMap<String, Option<String>>> {
    let dir = icons_dir();
    // 锁内只读缓存:图标提取(PE/icns 解析 + PNG 缩放编码写盘)是重 IO,必须在锁外执行,
    // 否则冷缓存时长时间持有全局唯一 DB 连接,阻塞 hidden/script 等其他命令(数百 ms 级)
    let cache: IconCache = {
        let conn = db.0.lock().unwrap();
        match db::get_setting(&conn, ICON_CACHE_SETTING_KEY)? {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => IconCache::new(),
        }
    };
    let (result, cache, dirty) = tokio::task::spawn_blocking(move || extract_all(dir, cache))
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    if dirty {
        let conn = db.0.lock().unwrap();
        let json = serde_json::to_string(&cache).unwrap_or_else(|_| "{}".into());
        db::set_setting(&conn, ICON_CACHE_SETTING_KEY, &json)?;
    }
    Ok(result)
}

/// 全部 kind 的提取流程(阻塞 IO,由 spawn_blocking 承载):建目录 → 逐 kind 解析/提取/写 PNG
fn extract_all(
    dir: PathBuf,
    mut cache: IconCache,
) -> (HashMap<String, Option<String>>, IconCache, bool) {
    let _ = std::fs::create_dir_all(&dir);
    let mut dirty = false;
    let mut result = HashMap::new();
    for kind in ALL_KINDS {
        let path = icon_for_kind(kind, &dir, &mut cache, &mut dirty);
        result.insert(
            kind_id(kind),
            path.map(|p| p.to_string_lossy().into_owned()),
        );
    }
    (result, cache, dirty)
}

// ---------------------------------------------------------------------------
// 图标源解析(平台相关)
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

/// 解析图标源候选(按优先级排序;提取失败会尝试下一个)
#[cfg(windows)]
fn resolve_icon_candidates(kind: EditorKind) -> Vec<PathBuf> {
    match kind {
        EditorKind::Explorer => vec![system_root().join("explorer.exe")],
        EditorKind::Terminal => {
            // wt 优先:AppX 执行别名是 reparse stub,读取/解析会失败,
            // 此时由候选链回退到 cmd.exe 的图标
            let mut candidates: Vec<PathBuf> =
                open::find_wt().into_iter().map(PathBuf::from).collect();
            candidates.push(system_root().join("System32").join("cmd.exe"));
            candidates
        }
        _ => open::cli_command(kind)
            .and_then(resolve_exe_from_cli)
            .into_iter()
            .collect(),
    }
}

#[cfg(target_os = "macos")]
fn resolve_icon_candidates(kind: EditorKind) -> Vec<PathBuf> {
    match kind {
        EditorKind::Explorer => {
            return icns_in_bundle(Path::new("/System/Library/CoreServices/Finder.app"))
                .into_iter()
                .collect()
        }
        EditorKind::Terminal => {
            return icns_in_bundle(Path::new("/System/Applications/Utilities/Terminal.app"))
                .into_iter()
                .collect()
        }
        _ => {}
    }
    let mut candidates = Vec::new();
    if let Some(cli) = open::cli_command(kind) {
        // CLI shim 通常是指向 *.app/Contents/... 的符号链接,顺链接反推 bundle
        if let Some(bundle) = bundle_from_cli(cli) {
            candidates.extend(icns_in_bundle(&bundle));
        }
        // 兜底:按 App 名前缀扫标准应用目录(兼容 Toolbox 命名,如 "IntelliJ IDEA Ultimate.app")
        if candidates.is_empty() {
            if let Some(prefix) = mac_app_prefix(kind) {
                let mut dirs = vec![PathBuf::from("/Applications")];
                if let Some(home) = std::env::var_os("HOME") {
                    dirs.push(PathBuf::from(home).join("Applications"));
                }
                for dir in dirs {
                    if let Some(bundle) = find_app_by_prefix(&dir, prefix) {
                        candidates.extend(icns_in_bundle(&bundle));
                        break;
                    }
                }
            }
        }
    }
    candidates
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn resolve_icon_candidates(_kind: EditorKind) -> Vec<PathBuf> {
    Vec::new()
}

/// Windows:where 查 CLI 路径;.exe 直接用,.cmd/.bat shim 读脚本内容找真实 exe
#[cfg(windows)]
fn resolve_exe_from_cli(cli: &str) -> Option<PathBuf> {
    let out = open::hidden(std::process::Command::new("where"))
        .arg(cli)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut shims = Vec::new();
    for line in stdout.lines() {
        let path = PathBuf::from(line.trim());
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("exe") => return Some(path),
            Some("cmd" | "bat") => shims.push(path),
            _ => {}
        }
    }
    shims.iter().find_map(|s| exe_from_shim(s))
}

#[cfg(windows)]
fn exe_from_shim(shim: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?;
    exe_refs_in_script(&content, dir)
        .into_iter()
        .find(|p| p.is_file())
}

/// 从 cmd/bat shim 脚本内容中提取 .exe 引用(按出现顺序),展开 %~dp0 与 %ENV%。
/// 覆盖 VS Code 系的 "%~dp0..\Code.exe" 与 JetBrains Toolbox 生成脚本的绝对路径。
#[cfg(any(windows, test))]
fn exe_refs_in_script(content: &str, shim_dir: &Path) -> Vec<PathBuf> {
    let lower = content.to_ascii_lowercase();
    let bytes = content.as_bytes();
    let mut refs = Vec::new();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(".exe") {
        let end = from + pos + ".exe".len();
        from = end;
        // 回看 token 起点:引号 / 空白 / cmd 元字符
        let mut start = end;
        while start > 0 {
            let c = bytes[start - 1] as char;
            if c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '|' | '&' | '<' | '>' | '(' | ')' | ';' | ','
                )
            {
                break;
            }
            start -= 1;
        }
        if let Some(path) = expand_shim_token(&content[start..end], shim_dir) {
            refs.push(path);
        }
    }
    refs
}

/// 展开 shim token 中的 %~dp0(shim 所在目录,cmd 语义带尾部反斜杠)与 %VAR% 环境变量;
/// % 配对失败(如命令行里的 %*)说明不是路径 token,丢弃
#[cfg(any(windows, test))]
fn expand_shim_token(token: &str, shim_dir: &Path) -> Option<PathBuf> {
    let mut out = String::with_capacity(token.len());
    let dp0 = format!(
        "{}\\",
        shim_dir.to_string_lossy().trim_end_matches(['/', '\\'])
    );
    let mut rest = token;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(stripped) = rest.strip_prefix("~dp0") {
            out.push_str(&dp0);
            rest = stripped;
            continue;
        }
        let Some(j) = rest.find('%') else { return None };
        let name = &rest[..j];
        if name.is_empty() {
            return None;
        }
        out.push_str(&std::env::var(name).ok()?);
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    let path = PathBuf::from(out);
    // 相对引用基于 shim 目录解析
    Some(if path.is_absolute() {
        path
    } else {
        shim_dir.join(path)
    })
}

/// macOS:which 查 CLI 并解析符号链接,从目标路径截取 .app bundle 根
#[cfg(target_os = "macos")]
fn bundle_from_cli(cli: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("which").arg(cli).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    if first.is_empty() {
        return None;
    }
    let resolved = std::fs::canonicalize(first).ok()?;
    bundle_root_from_path(&resolved)
}

/// 从 app 内部路径截取 .app bundle 根:CLI 通常在 *.app/Contents/... 下
#[cfg(any(target_os = "macos", test))]
fn bundle_root_from_path(path: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for comp in path.components() {
        root.push(comp.as_os_str());
        if comp
            .as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".app")
        {
            return Some(root);
        }
    }
    None
}

/// macOS 各编辑器 .app 名前缀(用于 /Applications 兜底扫描)
#[cfg(target_os = "macos")]
fn mac_app_prefix(kind: EditorKind) -> Option<&'static str> {
    Some(match kind {
        EditorKind::Vscode => "Visual Studio Code",
        EditorKind::Cursor => "Cursor",
        EditorKind::Windsurf => "Windsurf",
        EditorKind::Trae => "Trae",
        EditorKind::Vscodium => "VSCodium",
        EditorKind::Zed => "Zed",
        EditorKind::Sublime => "Sublime Text",
        EditorKind::Idea => "IntelliJ IDEA",
        EditorKind::Webstorm => "WebStorm",
        EditorKind::Goland => "GoLand",
        EditorKind::Pycharm => "PyCharm",
        EditorKind::Clion => "CLion",
        EditorKind::Rustrover => "RustRover",
        _ => return None,
    })
}

/// 在应用目录下按名称前缀找 .app(大小写不敏感;同名多个时取名字最短的,贴近官方安装名)
#[cfg(target_os = "macos")]
fn find_app_by_prefix(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy().to_ascii_lowercase();
                    n.starts_with(&prefix.to_ascii_lowercase()) && n.ends_with(".app")
                })
                .unwrap_or(false)
                && p.is_dir()
        })
        .collect();
    matches.sort_by_key(|p| p.as_os_str().len());
    matches.into_iter().next()
}

/// macOS:读 bundle 的 Info.plist 取 CFBundleIconFile,定位 Contents/Resources 下的 .icns
#[cfg(target_os = "macos")]
fn icns_in_bundle(bundle: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(bundle.join("Contents").join("Info.plist")).ok()?;
    let icon = icon_file_from_info_plist(&bytes)?;
    let path = bundle.join("Contents").join("Resources").join(icon);
    path.is_file().then_some(path)
}

/// 从 Info.plist 内容解析 CFBundleIconFile;值不带 .icns 后缀时补上
#[cfg(any(target_os = "macos", test))]
fn icon_file_from_info_plist(bytes: &[u8]) -> Option<String> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()?;
    let name = value
        .as_dictionary()?
        .get("CFBundleIconFile")?
        .as_string()?
        .trim();
    if name.is_empty() {
        return None;
    }
    Some(if name.to_ascii_lowercase().ends_with(".icns") {
        name.to_string()
    } else {
        format!("{name}.icns")
    })
}

// ---------------------------------------------------------------------------
// 图标解码(按文件扩展名分发,纯解析逻辑与平台无关,便于跨平台单测)
// ---------------------------------------------------------------------------

/// 提取图标源为 RGBA 像素(w, h, rgba)
fn extract_rgba(source: &Path) -> Option<(u32, u32, Vec<u8>)> {
    if source
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("icns"))
    {
        extract_from_icns(source)
    } else {
        extract_from_pe(source)
    }
}

/// exe 内嵌图标:pelite 读第一个 GROUP_ICON(即资源管理器展示的主图标)。
/// 不用 pelite 的整组 ICO 重组:部分 exe(如 Trae)GRPICONDIRENTRY 的 dwBytesInRes
/// 与真实数据大小不符,重组后偏移错位导致大图解码失败——改为按尺寸降序逐条取
/// 真实资源数据(nId → DataEntry)自拼单条 ICO,解码成功即返回。
fn extract_from_pe(exe: &Path) -> Option<(u32, u32, Vec<u8>)> {
    use pelite::resources::group::image::GRPICONDIRENTRY;

    /// 组图标条目的像素数(宽/高字节为 0 表示 256)
    fn entry_pixels(e: &GRPICONDIRENTRY) -> u32 {
        let w = if e.bWidth == 0 {
            256
        } else {
            u32::from(e.bWidth)
        };
        let h = if e.bHeight == 0 {
            256
        } else {
            u32::from(e.bHeight)
        };
        w * h
    }

    /// 单条 ICO:6 字节 ICONDIR + 16 字节 ICONDIRENTRY + 图像数据;
    /// dwBytesInRes 用真实数据长度,规避组条目里的错误值
    fn build_single_icon_ico(e: &GRPICONDIRENTRY, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(22 + data.len());
        out.extend_from_slice(&[0, 0, 1, 0, 1, 0]); // reserved, type=icon, count=1
        out.extend_from_slice(&[e.bWidth, e.bHeight, e.bColorCount, 0]);
        out.extend_from_slice(&e.wPlanes.to_le_bytes());
        out.extend_from_slice(&e.wBitCount.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    let bytes = std::fs::read(exe).ok()?;
    let pe = pelite::PeFile::from_bytes(&bytes).ok()?;
    let resources = pe.resources().ok()?;
    let (_, group) = resources.icons().find_map(|r| r.ok())?;
    let mut entries = group.entries().to_vec();
    entries.sort_by_key(|e| std::cmp::Reverse(entry_pixels(e)));
    for entry in &entries {
        let Ok(data) = group.image(entry.nId) else {
            continue;
        };
        let ico_bytes = build_single_icon_ico(entry, data);
        let Ok(dir) = ico::IconDir::read(std::io::Cursor::new(ico_bytes)) else {
            continue;
        };
        let Some(first) = dir.entries().first() else {
            continue;
        };
        if let Ok(img) = first.decode() {
            return Some((img.width(), img.height(), img.rgba_data().to_vec()));
        }
    }
    None
}

/// .icns 图标:解码全部可用元素,取最大尺寸,统一转 RGBA
fn extract_from_icns(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let family = icns::IconFamily::read(std::io::Cursor::new(bytes)).ok()?;
    let mut best: Option<icns::Image> = None;
    for ty in family.available_icons() {
        if let Ok(img) = family.get_icon_with_type(ty) {
            let better = match &best {
                None => true,
                Some(b) => img.width() * img.height() > b.width() * b.height(),
            };
            if better {
                best = Some(img);
            }
        }
    }
    let img = best?.convert_to(icns::PixelFormat::RGBA);
    Some((img.width(), img.height(), img.data().to_vec()))
}

/// RGBA 像素缩放到 ICON_SIZE(源更小则不放大)并写 PNG
fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> AppResult<()> {
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec())
        .ok_or_else(|| AppError::coded(ErrorCode::IoError, "icon pixel buffer size mismatch"))?;
    if w > ICON_SIZE || h > ICON_SIZE {
        image::imageops::resize(
            &img,
            ICON_SIZE,
            ICON_SIZE,
            image::imageops::FilterType::Lanczos3,
        )
        .save(path)
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))
    } else {
        img.save(path)
            .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_id_matches_serde() {
        // kind_id 必须与 EditorKind 的 serde lowercase id 一致(前端按 id 取图标)
        for kind in ALL_KINDS {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind_id(kind)));
        }
    }

    #[test]
    fn cache_hit_rules() {
        let entry = IconCacheEntry {
            target: r"C:\a\code.exe".into(),
            mtime: 10,
        };
        assert!(cache_hit(Some(&entry), r"C:\a\code.exe", 10, true));
        // mtime 变化(编辑器升级)→ 重新提取
        assert!(!cache_hit(Some(&entry), r"C:\a\code.exe", 11, true));
        // 目标路径变化 → 重新提取
        assert!(!cache_hit(Some(&entry), r"C:\b\code.exe", 10, true));
        // PNG 被删 → 重新提取
        assert!(!cache_hit(Some(&entry), r"C:\a\code.exe", 10, false));
        assert!(!cache_hit(None, r"C:\a\code.exe", 10, true));
    }

    #[test]
    fn exe_refs_vscode_style_shim() {
        let shim_dir = Path::new(r"C:\Tools\Microsoft VS Code\bin");
        let content = "@echo off\r\nsetlocal\r\nset ELECTRON_RUN_AS_NODE=1\r\n\"%~dp0..\\Code.exe\" \"%~dp0..\\resources\\app\\out\\cli.js\" %*\r\nendlocal\r\n";
        let refs = exe_refs_in_script(content, shim_dir);
        assert_eq!(refs, vec![shim_dir.join("..\\Code.exe")]);
    }

    #[test]
    fn exe_refs_jetbrains_toolbox_style() {
        let shim_dir = Path::new(r"C:\Tools\scripts");
        let content = "@echo off\r\n\"C:\\Users\\me\\AppData\\Local\\JetBrains\\Toolbox\\apps\\IDEA-U\\ch-0\\233\\bin\\idea64.exe\" %*\r\n";
        let refs = exe_refs_in_script(content, shim_dir);
        assert_eq!(
            refs,
            vec![PathBuf::from(
                r"C:\Users\me\AppData\Local\JetBrains\Toolbox\apps\IDEA-U\ch-0\233\bin\idea64.exe"
            )]
        );
    }

    #[test]
    fn exe_refs_expands_env_vars() {
        // 用进程必有的 SystemRoot 做环境变量展开用例
        let root = std::env::var("SystemRoot").unwrap();
        let shim_dir = Path::new(r"C:\shim");
        let content = r#""%SystemRoot%\System32\cmd.exe" %*"#;
        let refs = exe_refs_in_script(content, shim_dir);
        assert_eq!(
            refs,
            vec![PathBuf::from(root).join("System32").join("cmd.exe")]
        );
    }

    #[test]
    fn exe_refs_relative_and_garbage() {
        let shim_dir = Path::new(r"C:\shim");
        // 无 exe 引用
        assert!(exe_refs_in_script("@echo off\r\necho hello\r\n", shim_dir).is_empty());
        // 裸相对引用基于 shim 目录
        let refs = exe_refs_in_script("start app.exe", shim_dir);
        assert_eq!(refs, vec![shim_dir.join("app.exe")]);
    }

    #[test]
    fn bundle_root_from_app_inner_path() {
        let p = Path::new("/Applications/Visual Studio Code.app/Contents/Resources/bin/code");
        assert_eq!(
            bundle_root_from_path(p),
            Some(PathBuf::from("/Applications/Visual Studio Code.app"))
        );
        // 深层 Toolbox 路径
        let p = Path::new("/Users/me/Applications/IntelliJ IDEA Ultimate.app/Contents/MacOS/idea");
        assert_eq!(
            bundle_root_from_path(p),
            Some(PathBuf::from(
                "/Users/me/Applications/IntelliJ IDEA Ultimate.app"
            ))
        );
        // 不在 .app 内
        assert_eq!(
            bundle_root_from_path(Path::new("/usr/local/bin/code")),
            None
        );
    }

    #[test]
    fn info_plist_icon_file() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIconFile</key><string>Code</string>
</dict></plist>"#;
        assert_eq!(
            icon_file_from_info_plist(xml),
            Some("Code.icns".to_string())
        );
        // 已带后缀
        let xml2 = br#"<?xml version="1.0"?><plist version="1.0"><dict>
<key>CFBundleIconFile</key><string>AppIcon.icns</string>
</dict></plist>"#;
        assert_eq!(
            icon_file_from_info_plist(xml2),
            Some("AppIcon.icns".to_string())
        );
        // 缺 key / 空值
        let xml3 = br#"<?xml version="1.0"?><plist version="1.0"><dict></dict></plist>"#;
        assert_eq!(icon_file_from_info_plist(xml3), None);
    }

    /// 端到端(仅 Windows):从系统 exe 提取真实图标,验证像素尺寸与 RGBA 长度
    #[cfg(windows)]
    #[test]
    fn extract_icon_from_system_exe() {
        let exe = system_root().join("System32").join("cmd.exe");
        if !exe.is_file() {
            return;
        }
        let (w, h, rgba) = extract_rgba(&exe).expect("cmd.exe 应能提取图标");
        assert!(w > 0 && h > 0);
        assert_eq!(rgba.len() as u64, u64::from(w) * u64::from(h) * 4);
    }

    /// 端到端(仅 Windows):缩小写 PNG
    #[cfg(windows)]
    #[test]
    fn write_png_downscales() {
        let (w, h, rgba) = (128, 128, vec![255u8; 128 * 128 * 4]);
        let dir = std::env::temp_dir().join(format!("repomeow-icon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("t.png");
        write_png(&png, w, h, &rgba).unwrap();
        let decoded = image::open(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (ICON_SIZE, ICON_SIZE));
        std::fs::remove_dir_all(&dir).ok();
    }
}
