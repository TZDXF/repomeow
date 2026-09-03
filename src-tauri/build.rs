fn main() {
    // 应用清单(bin 与 lib 单元测试二进制需要同一份 comctl32 v6 声明,
    // 见 windows/app.manifest 注释)统一由下方链接参数嵌入,
    // 因此关闭 tauri-build 自带的清单嵌入,避免资源重复(CVT1100/LNK1123)。
    // 图标与版本信息资源仍由 tauri-build 生成,不受影响。
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
    )
    .expect("failed to run tauri-build");

    // cargo:rustc-link-arg-tests 不覆盖 lib 单元测试二进制(cargo 校验还要求
    // 存在 tests/ 集成测试目标),只能用 rustc-link-arg 对全目标统一嵌入;
    // 前提是上方已关闭 tauri-build 的清单,二者不可同时开启。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows/app.manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
