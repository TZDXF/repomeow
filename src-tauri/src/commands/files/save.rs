use std::path::Path;

use crate::error::{AppError, AppResult, ErrorCode};

/// 写入文本到指定路径(供 Markdown 代码块/表格"下载"按钮走 Tauri save dialog 后调用)。
/// 内容上限为 512KB,目标路径不能为空且父目录必须存在。
/// 写入会创建或覆盖目标文件。
pub(super) const SAVE_TEXT_MAX_BYTES: usize = 512 * 1024;

pub(super) fn save_text_file(path: String, content: String) -> AppResult<()> {
    if path.trim().is_empty() {
        return Err(AppError::coded(ErrorCode::SavePathRequired, ""));
    }
    if content.len() > SAVE_TEXT_MAX_BYTES {
        return Err(AppError::coded(
            ErrorCode::SaveContentTooLarge,
            SAVE_TEXT_MAX_BYTES.to_string(),
        ));
    }
    // 父目录必须存在
    let p = Path::new(&path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(AppError::coded(
                ErrorCode::SaveParentDirMissing,
                parent.display().to_string(),
            ));
        }
    }
    std::fs::write(p, content.as_bytes())?;
    Ok(())
}
