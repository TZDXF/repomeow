use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use super::repository::get;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::Project;
use crate::time_util::now_ts;

/// 一次目录移动的校验结果(源/目标路径)
pub(super) struct MovePlan {
    pub(super) src: PathBuf,
    pub(super) target: PathBuf,
    pub(super) target_str: String,
}

/// 校验移动参数并计算目标路径(不触碰磁盘)
pub(super) fn prepare_move(
    conn: &Connection,
    id: i64,
    target_parent: &str,
    dir_name: &str,
) -> AppResult<MovePlan> {
    let project = get(conn, id)?;
    let src = PathBuf::from(&project.path);
    if !src.is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, &project.path));
    }
    let parent = Path::new(target_parent.trim());
    if !parent.is_dir() {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            target_parent.trim(),
        ));
    }
    let dir_name = dir_name.trim();
    if dir_name.is_empty()
        || dir_name == "."
        || dir_name == ".."
        || dir_name.contains('/')
        || dir_name.contains('\\')
    {
        return Err(AppError::coded(ErrorCode::MoveInvalidDirName, dir_name));
    }
    let target = parent.join(dir_name);
    // 目标路径归一化后再比较与落库:用户输入的 parent 可能是正斜杠风格,
    // 与库里登记的反斜杠路径字面不等会让"移动到自身位置"绕过 MoveSameLocation 检查
    let target_str = crate::path_util::clean(&target)
        .to_string_lossy()
        .to_string();
    // Windows 文件系统大小写不敏感,统一按忽略大小写判断"位置未变化"
    if target_str.eq_ignore_ascii_case(&project.path) {
        return Err(AppError::coded(ErrorCode::MoveSameLocation, ""));
    }
    if target.starts_with(&src) {
        return Err(AppError::coded(ErrorCode::MoveInsideSelf, ""));
    }
    if target.exists() {
        return Err(AppError::coded(
            ErrorCode::MoveTargetExists,
            target.to_string_lossy().to_string(),
        ));
    }
    // 目标路径已被其他项目登记时提前报错,避免移动后数据库唯一键冲突
    let registered = conn
        .query_row(
            "SELECT id FROM projects WHERE path = ?1 AND id != ?2",
            params![target_str, id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    if registered.is_some() {
        return Err(AppError::coded(ErrorCode::ProjectPathConflict, target_str));
    }
    Ok(MovePlan {
        src,
        target,
        target_str,
    })
}

/// 磁盘移动:同盘直接 rename;跨盘退回"复制 + 删除源"
pub(super) fn move_folder(src: &Path, target: &Path) -> AppResult<()> {
    match std::fs::rename(src, target) {
        Ok(()) => Ok(()),
        // Windows ERROR_NOT_SAME_DEVICE(17) / Unix EXDEV(18)
        Err(e) if matches!(e.raw_os_error(), Some(17) | Some(18)) => {
            copy_across_devices(src, target)
        }
        Err(e) => Err(AppError::Io(e)),
    }
}

/// 跨盘移动(Windows):robocopy 复制(保留 junction/symlink 结构,/SL)成功后删除源。
/// 不用 /MOVE:复制失败时源目录保持完整;目标半成品尽力清理。
#[cfg(windows)]
fn copy_across_devices(src: &Path, target: &Path) -> AppResult<()> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("robocopy")
        .arg(src)
        .arg(target)
        .args([
            "/E",
            "/SL",
            "/COPY:DAT",
            "/DCOPY:T",
            "/R:1",
            "/W:1",
            "/NFL",
            "/NDL",
            "/NJH",
            "/NJS",
            "/NP",
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(AppError::Io)?;
    // robocopy 退出码是位标记,< 8 均表示成功(0=无变化 1=已复制 2/4=额外/不匹配文件)
    let code = output.status.code().unwrap_or(16);
    if code >= 8 {
        let _ = std::fs::remove_dir_all(target);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let tail: String = stdout
            .chars()
            .rev()
            .take(200)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(AppError::coded(
            ErrorCode::MoveRobocopyFailed,
            format!("code={code} tail={tail}"),
        ));
    }
    std::fs::remove_dir_all(src).map_err(AppError::Io)
}

/// 跨盘移动(非 Windows):递归复制(符号链接按链接重建)成功后删除源
#[cfg(not(windows))]
fn copy_across_devices(src: &Path, target: &Path) -> AppResult<()> {
    if let Err(e) = copy_dir_recursive(src, target) {
        let _ = std::fs::remove_dir_all(target);
        return Err(AppError::Io(e));
    }
    std::fs::remove_dir_all(src).map_err(AppError::Io)
}

#[cfg(not(windows))]
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_symlink() {
            let link = std::fs::read_link(entry.path())?;
            std::os::unix::fs::symlink(&link, &to)?;
        } else if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// 落库:更新登记路径;失败时尽力把文件夹移回原位
pub(super) fn apply_move(conn: &Connection, id: i64, plan: &MovePlan) -> AppResult<()> {
    if let Err(e) = conn.execute(
        "UPDATE projects SET path = ?1, updated_at = ?2 WHERE id = ?3",
        params![plan.target_str, now_ts(), id],
    ) {
        let _ = std::fs::rename(&plan.target, &plan.src);
        return Err(AppError::Db(e));
    }
    Ok(())
}

/// 应用内移动项目目录:把项目文件夹移动到新的父目录下(可同时改名),并更新登记路径。
/// 同盘直接 rename;跨盘自动退回"复制 + 删除源"(大目录耗时较长,由异步命令承载)。
// 命令端为不持锁移动拆成了 prepare_move/move_folder/apply_move 三阶段,此组合函数供测试使用
#[allow(dead_code)]
pub fn move_dir(
    conn: &Connection,
    id: i64,
    target_parent: &str,
    dir_name: &str,
) -> AppResult<Project> {
    let plan = prepare_move(conn, id, target_parent, dir_name)?;
    move_folder(&plan.src, &plan.target)?;
    apply_move(conn, id, &plan)?;
    get(conn, id)
}
