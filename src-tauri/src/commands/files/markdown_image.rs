use std::path::Path;

use crate::error::{AppError, AppResult, ErrorCode};

/// 返回真实路径供 asset 协议读取；仅接受项目根目录下的普通文件。
/// 校验与 asset 协议实际打开之间仍存在 TOCTOU 窗口。
pub(super) fn resolve_markdown_image(root: String, image_path: String) -> AppResult<String> {
    let invalid = || AppError::coded(ErrorCode::InvalidPath, "invalid markdown image path");
    let root_path = Path::new(&root);
    let image = Path::new(&image_path);
    if !root_path.is_absolute() || !image.is_absolute() {
        return Err(invalid());
    }
    let canonical_root = root_path.canonicalize().map_err(|_| invalid())?;
    if !canonical_root.is_dir() {
        return Err(invalid());
    }
    let canonical_image = image.canonicalize().map_err(|_| invalid())?;
    if !canonical_image.starts_with(&canonical_root) || !canonical_image.is_file() {
        return Err(invalid());
    }
    canonical_image
        .to_str()
        .map(str::to_owned)
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        dir: PathBuf,
        root: PathBuf,
        outside: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "repomeow-md-image-{}-{}",
                std::process::id(),
                crate::time_util::now_ts_nanos()
            ));
            let root = dir.join("project");
            let outside = dir.join("project-other");
            fs::create_dir_all(root.join("docs")).unwrap();
            fs::create_dir_all(&outside).unwrap();
            fs::write(root.join("docs/image.png"), b"image").unwrap();
            fs::write(outside.join("secret.png"), b"secret").unwrap();
            Self { dir, root, outside }
        }
        fn resolve(&self, image: &Path) -> AppResult<String> {
            resolve_markdown_image(
                self.root.to_str().unwrap().into(),
                image.to_str().unwrap().into(),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn resolves_real_file_and_rejects_non_files_and_escaped_paths() {
        let f = Fixture::new();
        let image = f.root.join("docs/image.png");
        assert_eq!(
            PathBuf::from(f.resolve(&image).unwrap()),
            image.canonicalize().unwrap()
        );
        assert!(f.resolve(&f.root.join("docs")).is_err());
        assert!(f.resolve(&f.root.join("missing.png")).is_err());
        assert!(f
            .resolve(&f.root.join("../project-other/secret.png"))
            .is_err());
        assert!(f.resolve(&f.outside.join("secret.png")).is_err());
        assert!(resolve_markdown_image(".".into(), image.to_string_lossy().into_owned()).is_err());
        assert!(resolve_markdown_image(
            f.root.to_string_lossy().into_owned(),
            "docs/image.png".into()
        )
        .is_err());
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn symlinks_cannot_escape_and_in_root_links_return_canonical_target() {
        let f = Fixture::new();
        #[cfg(unix)]
        use std::os::unix::fs::symlink as symlink_dir;
        #[cfg(windows)]
        use std::os::windows::fs::symlink_dir;
        // Windows 未启用开发者模式时创建符号链接可能需要管理员权限。
        if let Err(err) = symlink_dir(&f.outside, f.root.join("outside-link")) {
            #[cfg(windows)]
            if err.raw_os_error() == Some(1314) {
                eprintln!("跳过符号链接测试:系统未授予创建符号链接权限");
                return;
            }
            panic!("创建符号链接失败: {err}");
        }
        assert!(f.resolve(&f.root.join("outside-link/secret.png")).is_err());
        symlink_dir(f.root.join("docs"), f.root.join("inside-link")).unwrap();
        let actual = f.root.join("docs/image.png").canonicalize().unwrap();
        assert_eq!(
            PathBuf::from(f.resolve(&f.root.join("inside-link/image.png")).unwrap()),
            actual
        );
        // 根本身也是符号链接时,必须对根做 canonicalize 后比较。
        symlink_dir(&f.root, f.dir.join("root-link")).unwrap();
        let root_link = f.dir.join("root-link");
        assert_eq!(
            PathBuf::from(
                resolve_markdown_image(
                    root_link.to_str().unwrap().into(),
                    root_link
                        .join("inside-link/image.png")
                        .to_str()
                        .unwrap()
                        .into()
                )
                .unwrap()
            ),
            actual
        );
    }
}
