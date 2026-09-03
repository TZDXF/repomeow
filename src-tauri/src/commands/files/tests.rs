use super::*;
use std::fs;
use std::path::Path;

use crate::error::ErrorCode;
use crate::models::ComposePort;

/// 建一个带唯一名字的临时目录,返回路径字符串
fn temp_project_dir(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "repomeow-files-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().to_string()
}

#[test]
fn list_project_files_lists_single_level_and_marks_ignored() {
    let dir = temp_project_dir("list");
    let p = Path::new(&dir);
    fs::write(p.join(".gitignore"), "logs/\n").unwrap();
    fs::create_dir_all(p.join("node_modules/dep")).unwrap();
    fs::write(p.join("node_modules/dep/package.json"), "{}").unwrap();
    fs::create_dir_all(p.join("logs")).unwrap();
    fs::write(p.join("logs/app.log"), "x").unwrap();
    fs::create_dir_all(p.join("empty")).unwrap();
    fs::write(p.join(".env"), "A=1").unwrap();
    fs::write(p.join("src.rs"), "fn main() {}").unwrap();

    // 根层:只有直接子项,文件与目录(含空目录)都返回
    let entries = list_project_files(dir.clone(), None).unwrap();
    let by_path: std::collections::HashMap<&str, &ProjectFileEntry> =
        entries.iter().map(|e| (e.path.as_str(), e)).collect();
    for expected in [
        "node_modules",
        "logs",
        "empty",
        ".env",
        ".gitignore",
        "src.rs",
    ] {
        assert!(by_path.contains_key(expected), "缺少 {expected}");
    }
    assert!(
        !by_path.keys().any(|k| k.contains('/')),
        "根层不应出现嵌套路径"
    );
    assert!(by_path["node_modules"].is_dir && by_path["empty"].is_dir);
    assert!(!by_path["src.rs"].is_dir);
    // 被 .gitignore 排除的目录整体标 ignored;未排除的 node_modules 不标
    assert!(by_path["logs"].ignored);
    assert!(!by_path["node_modules"].ignored);
    assert!(!by_path[".env"].ignored && !by_path["src.rs"].ignored);

    // 子目录层
    let entries = list_project_files(dir.clone(), Some("logs".into())).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "logs/app.log");
    assert!(!entries[0].is_dir);
    assert!(entries[0].ignored, "父目录被排除时其内文件同样 ignored");

    // dir 越界 / 指向文件被拒绝
    assert!(list_project_files(dir.clone(), Some("../".into())).is_err());
    assert!(list_project_files(dir.clone(), Some("src.rs".into())).is_err());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_project_files_filters_and_limits() {
    let dir = temp_project_dir("filesearch");
    let p = Path::new(&dir);
    fs::write(p.join("App.vue"), "<template />").unwrap();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(p.join("src/app.ts"), "x").unwrap();
    fs::write(p.join("other.txt"), "x").unwrap();
    fs::create_dir_all(p.join("logs")).unwrap();
    fs::write(p.join("logs/app.log"), "x").unwrap();
    fs::write(p.join(".gitignore"), "logs/\n").unwrap();

    // 大小写不敏感;gitignore 排除的不参与;ignored/is_dir 恒为 false
    let r = search_project_files(dir.clone(), "app".into(), Some(50)).unwrap();
    let paths: Vec<&str> = r.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["App.vue", "src/app.ts"]);
    assert!(r.iter().all(|e| !e.ignored && !e.is_dir));

    // 空查询返回空
    assert!(search_project_files(dir.clone(), "  ".into(), None)
        .unwrap()
        .is_empty());
    // limit 生效
    assert_eq!(
        search_project_files(dir.clone(), "app".into(), Some(1))
            .unwrap()
            .len(),
        1
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn read_file_preview_text_binary_and_escape() {
    let dir = temp_project_dir("preview");
    let p = Path::new(&dir);
    fs::write(p.join("a.txt"), "hello").unwrap();
    // 前 8KB 内带 NUL → 二进制
    fs::write(p.join("b.bin"), b"AB\0CD").unwrap();
    fs::create_dir_all(p.join("sub")).unwrap();
    let outside = temp_project_dir("preview-outside");
    fs::write(Path::new(&outside).join("secret.txt"), "secret").unwrap();

    let r = read_file_preview(dir.clone(), "a.txt".into()).unwrap();
    assert_eq!(r.text.as_deref(), Some("hello"));
    assert!(!r.truncated);
    assert_eq!(r.token_count, Some(1));

    let r = read_file_preview(dir.clone(), "b.bin".into()).unwrap();
    assert!(r.text.is_none());
    assert_eq!(r.token_count, None);

    // 目录不是文件
    assert!(read_file_preview(dir.clone(), "sub".into()).is_err());
    // .. 越界读取项目外文件被拒绝
    let escape = format!(
        "../{}/secret.txt",
        Path::new(&outside).file_name().unwrap().to_string_lossy()
    );
    assert!(read_file_preview(dir.clone(), escape).is_err());
    // 不存在的文件
    assert!(read_file_preview(dir.clone(), "nope.txt".into()).is_err());

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn read_file_preview_truncates_on_utf8_boundary() {
    let dir = temp_project_dir("truncate");
    let p = Path::new(&dir);
    // 超出 512KB 的纯 ASCII + 边界处放多字节字符
    let mut content = "a".repeat(PREVIEW_MAX_BYTES as usize);
    content.push('中');
    content.push_str(&"b".repeat(64));
    fs::write(p.join("big.txt"), &content).unwrap();

    let r = read_file_preview(dir.clone(), "big.txt".into()).unwrap();
    assert!(r.truncated);
    let token_count = r.token_count.unwrap();
    let text = r.text.unwrap();
    assert!(text.len() <= PREVIEW_MAX_BYTES as usize);
    assert!(text.chars().all(|c| c == 'a'));
    assert!(token_count > crate::commands::usage::count_o200k_tokens(&text));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn readme_missing_and_found() {
    let dir = temp_project_dir("readme");
    let p = Path::new(&dir);

    assert!(read_readme(&dir).unwrap().is_none());

    fs::write(p.join("readme.md"), "# Hello").unwrap();
    assert_eq!(read_readme(&dir).unwrap().as_deref(), Some("# Hello"));

    // 优先级:README.md 高于 readme.md。
    // 注意先删 readme.md:Windows/macOS 大小写不敏感文件系统上,
    // 直接写 README.md 会覆盖同名文件但保留原有目录项大小写。
    fs::remove_file(p.join("readme.md")).unwrap();
    fs::write(p.join("README.md"), "# Priority").unwrap();
    assert_eq!(read_readme(&dir).unwrap().as_deref(), Some("# Priority"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn readme_rejects_missing_dir() {
    assert!(matches!(read_readme("D:/no/such/dir-xyz"),
                Err(ref e) if e.is_code(crate::error::ErrorCode::InvalidPath)));
}

#[test]
fn compose_scan_by_content_not_name() {
    let dir = temp_project_dir("compose-content");
    let p = Path::new(&dir);

    // 非标准文件名,但内容是 compose 格式 -> 识别
    fs::write(p.join("app.yml"), "services:\n  web:\n    image: nginx\n").unwrap();
    // 标准文件名但无 services -> 不识别
    fs::write(p.join("docker-compose.yml"), "name: demo\n").unwrap();
    // CI 配置(yml 但非 compose)-> 不识别
    fs::write(p.join("ci.yaml"), "on: push\njobs: {}\n").unwrap();
    // 非法 YAML -> 不识别
    fs::write(p.join("broken.yml"), "services: [not a map").unwrap();
    // 非 yml 文件不参与
    fs::write(p.join("services.txt"), "services:\n  x: {}\n").unwrap();

    let files = scan_compose_files(dir.clone()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "app.yml");
    assert_eq!(files[0].file_name, "app.yml");
    assert_eq!(files[0].services[0].name, "web");
    assert!(files[0].services[0].ports.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compose_scan_nested_and_gitignored() {
    let dir = temp_project_dir("compose-nested");
    let p = Path::new(&dir);

    // 嵌套子目录中的 compose
    fs::create_dir_all(p.join("deploy/prod")).unwrap();
    fs::write(
        p.join("deploy/prod/stack.yaml"),
        "services:\n  api:\n    build: .\n  db:\n    image: postgres:16\n",
    )
    .unwrap();
    // 被 .gitignore 排除的目录不扫描
    fs::create_dir_all(p.join("ignored")).unwrap();
    fs::write(p.join("ignored/svc.yml"), "services:\n  x: {}\n").unwrap();
    fs::write(p.join(".gitignore"), "ignored/\n").unwrap();

    let files = scan_compose_files(dir.clone()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "deploy/prod/stack.yaml");
    let names: Vec<&str> = files[0].services.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["api", "db"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compose_scan_root_first_ordering() {
    let dir = temp_project_dir("compose-order");
    let p = Path::new(&dir);

    fs::create_dir_all(p.join("abc")).unwrap();
    fs::write(p.join("abc/x.yml"), "services:\n  a: {}\n").unwrap();
    // 根目录文件名字典序更大,但仍应排在前面
    fs::write(p.join("z.yml"), "services:\n  z: {}\n").unwrap();
    fs::write(p.join("a.yml"), "services:\n  a: {}\n").unwrap();

    let files = scan_compose_files(dir.clone()).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.yml", "z.yml", "abc/x.yml"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn maybe_compose_prefilter() {
    // 文件名含 compose 直接通过(哪怕内容暂未嗅探到顶层 services)
    assert!(maybe_compose_file("docker-compose.yml", "name: demo\n"));
    assert!(maybe_compose_file("COMPOSE.YAML", "x: 1\n"));
    // 顶层 services 键:标准写法、键后空格、CRLF 行尾都认
    assert!(maybe_compose_file("app.yml", "services:\n  web: {}\n"));
    assert!(maybe_compose_file("app.yml", "services :\n  web: {}\n"));
    assert!(maybe_compose_file(
        "app.yml",
        "---\nservices:\r\n  web: {}\n"
    ));
    // 缩进的嵌套 services(如 CI 配置)不算顶层键
    assert!(!maybe_compose_file(
        "ci.yaml",
        "jobs:\n  build:\n    services:\n      db: {}\n"
    ));
    // 注释与普通键不误判
    assert!(!maybe_compose_file("a.yml", "# services:\nx: 1\n"));
    assert!(!maybe_compose_file("a.yml", "serviceName: web\n"));
}

#[test]
fn save_text_file_writes_and_validates() {
    let dir = temp_project_dir("save-text");
    let p = Path::new(&dir);

    // 正常写入
    let target = p.join("out.csv");
    save_text_file(target.to_string_lossy().into_owned(), "a,b\n1,2\n".into()).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "a,b\n1,2\n");

    // 空路径报错
    assert!(matches!(
        save_text_file("".into(), "x".into()),
        Err(ref e) if e.is_code(ErrorCode::SavePathRequired)
    ));

    // 父目录不存在报错
    let bad = p.join("missing/out.txt");
    assert!(matches!(
        save_text_file(bad.to_string_lossy().into_owned(), "x".into()),
        Err(ref e) if e.is_code(ErrorCode::SaveParentDirMissing)
    ));

    // 超大内容报错
    let huge = "x".repeat(SAVE_TEXT_MAX_BYTES + 1);
    let target2 = p.join("huge.txt");
    assert!(matches!(
        save_text_file(target2.to_string_lossy().into_owned(), huge),
        Err(ref e) if e.is_code(ErrorCode::SaveContentTooLarge)
    ));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compose_extracts_accessible_ports() {
    let content = r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - "127.0.0.1:9090:90/tcp"
      - "53:53/udp"
      - "3000"
      - "8081-8082:81-82"
      - target: 443
        published: 8443
        protocol: tcp
      - target: 541
        published: 541
        protocol: udp
  api:
    image: app
    ports:
      - "${API_PORT:-8000}:8000"
  db:
    image: postgres
"#;
    let services = parse_compose(content).unwrap();
    assert_eq!(services.len(), 3);
    // web: 8080/9090/8443 可访问;udp、仅容器端口、端口段范围跳过;去重升序
    let port = |published: u16, target: u16| ComposePort { published, target };
    assert_eq!(
        services[0].ports,
        vec![port(8080, 80), port(8443, 443), port(9090, 90)]
    );
    assert_eq!(services[1].ports, vec![port(8000, 8000)]);
    assert!(services[2].ports.is_empty());
}

// ── 全文搜索 ────────────────────────────────────────────────────────────

/// 便捷:断言某文件的命中行号集合
fn hit_lines(outcome: &TextSearchOutcome, path: &str) -> Vec<u32> {
    outcome
        .hits
        .iter()
        .find(|h| h.path == path)
        .unwrap_or_else(|| panic!("缺少命中文件 {path}"))
        .lines
        .iter()
        .map(|l| l.line)
        .collect()
}

#[test]
fn search_project_text_modes() {
    let dir = temp_project_dir("search-modes");
    let p = Path::new(&dir);
    // 默认大小写不敏感:第 1、2 行命中
    fs::write(p.join("a.txt"), "Hello world\nhello WORLD\nbye\n").unwrap();

    let r = search_project_text(
        dir.clone(),
        "hello".into(),
        false,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    assert_eq!(hit_lines(&r, "a.txt"), vec![1, 2]);
    assert_eq!(r.hits[0].count, 2);
    assert!(!r.truncated);

    // 大小写敏感:仅第 2 行(hello WORLD)
    let r = search_project_text(
        dir.clone(),
        "hello".into(),
        true,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    assert_eq!(hit_lines(&r, "a.txt"), vec![2]);

    // 全字匹配:"world" 不命中 "worldwide"
    fs::write(p.join("b.txt"), "world\nworldwide\n").unwrap();
    let r = search_project_text(
        dir.clone(),
        "world".into(),
        false,
        true,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    assert_eq!(hit_lines(&r, "b.txt"), vec![1]);

    // 正则模式 + 全字组合:\b(?:c.t)\b
    fs::write(p.join("c.txt"), "cat cut\nconcat\n").unwrap();
    let r = search_project_text(
        dir.clone(),
        "c.t".into(),
        false,
        true,
        true,
        "".into(),
        "".into(),
    )
    .unwrap();
    assert_eq!(hit_lines(&r, "c.txt"), vec![1]);

    // 同行多次匹配计入 count,行只出现一次
    fs::write(p.join("d.txt"), "ab ab ab\n").unwrap();
    let r = search_project_text(
        dir.clone(),
        "ab".into(),
        false,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    assert_eq!(hit_lines(&r, "d.txt"), vec![1]);
    let d = r.hits.iter().find(|h| h.path == "d.txt").unwrap();
    assert_eq!(d.count, 3);
    assert_eq!(d.lines.len(), 1);

    // 空查询返回空结果
    let r = search_project_text(
        dir.clone(),
        "  ".into(),
        false,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    assert!(r.hits.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_project_text_scope_and_binary() {
    let dir = temp_project_dir("search-scope");
    let p = Path::new(&dir);
    fs::write(p.join(".env"), "SECRET_TOKEN=x\n").unwrap();
    fs::write(p.join("b.bin"), b"to\0ken").unwrap();
    fs::create_dir_all(p.join("node_modules/dep")).unwrap();
    fs::write(p.join("node_modules/dep/token.txt"), "token\n").unwrap();
    fs::create_dir_all(p.join("logs")).unwrap();
    fs::write(p.join("logs/token.log"), "token\n").unwrap();
    fs::write(p.join(".gitignore"), "logs/\n").unwrap();
    fs::create_dir_all(p.join(".git")).unwrap();
    fs::write(p.join(".git/token"), "token\n").unwrap();

    let r = search_project_text(
        dir.clone(),
        "token".into(),
        false,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    let paths: Vec<&str> = r.hits.iter().map(|h| h.path.as_str()).collect();
    // 隐藏文件可搜;二进制、node_modules、git 忽略目录、.git 内部跳过
    assert_eq!(paths, vec![".env"]);
    // .gitignore 自身无命中,不出现在结果里

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_project_text_long_line_window() {
    let dir = temp_project_dir("search-window");
    let p = Path::new(&dir);
    let mut line = "x".repeat(3000);
    line.push_str("NEEDLE");
    line.push_str(&"y".repeat(100));
    fs::write(p.join("min.js"), &line).unwrap();

    let r = search_project_text(
        dir.clone(),
        "NEEDLE".into(),
        false,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    let text = &r.hits[0].lines[0].text;
    assert!(text.starts_with('…'), "超长行窗口应带前省略号:{text}");
    assert!(text.ends_with('…'), "超长行窗口应带后省略号:{text}");
    assert!(text.contains("NEEDLE"));
    assert!(text.len() < line.len());

    // 短行原样返回
    fs::write(p.join("short.txt"), "NEEDLE here\n").unwrap();
    let r = search_project_text(
        dir.clone(),
        "NEEDLE".into(),
        false,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    let short = r.hits.iter().find(|h| h.path == "short.txt").unwrap();
    assert_eq!(short.lines[0].text, "NEEDLE here");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_project_text_caps_truncated() {
    let dir = temp_project_dir("search-caps");
    let p = Path::new(&dir);
    // 单文件 1100 个匹配:达到 1000 上限后停止累计并置 truncated
    let content = (0..1100)
        .map(|i| format!("hit {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(p.join("many.txt"), &content).unwrap();

    let r = search_project_text(
        dir.clone(),
        "hit".into(),
        false,
        false,
        false,
        "".into(),
        "".into(),
    )
    .unwrap();
    assert!(r.truncated);
    let hit = &r.hits[0];
    assert_eq!(hit.count, SEARCH_MAX_MATCHES);
    assert_eq!(hit.lines.len(), SEARCH_MAX_MATCHES as usize);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_project_text_filters_by_include_and_exclude_globs() {
    let dir = temp_project_dir("search-globs");
    let p = Path::new(&dir);
    fs::create_dir_all(p.join("src/nested")).unwrap();
    fs::create_dir_all(p.join("tests")).unwrap();
    fs::create_dir_all(p.join("dist")).unwrap();
    fs::write(p.join("src/main.ts"), "needle\n").unwrap();
    fs::write(p.join("src/nested/worker.ts"), "needle\n").unwrap();
    fs::write(p.join("tests/main.ts"), "needle\n").unwrap();
    fs::write(p.join("dist/bundle.ts"), "needle\n").unwrap();
    fs::write(p.join("README.md"), "needle\n").unwrap();

    let r = search_project_text(
        dir.clone(),
        "needle".into(),
        false,
        false,
        false,
        "src/**/*.ts, README.md".into(),
        "src/nested/**".into(),
    )
    .unwrap();
    let paths: Vec<&str> = r.hits.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(paths, vec!["README.md", "src/main.ts"]);

    // 无斜杠模式按 VS Code Search view 语义匹配任意层级;
    // ./ 只匹配工作区根层文件。
    let r = search_project_text(
        dir.clone(),
        "needle".into(),
        false,
        false,
        false,
        "*.ts".into(),
        "".into(),
    )
    .unwrap();
    let paths: Vec<&str> = r.hits.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "dist/bundle.ts",
            "src/main.ts",
            "src/nested/worker.ts",
            "tests/main.ts"
        ]
    );

    let r = search_project_text(
        dir.clone(),
        "needle".into(),
        false,
        false,
        false,
        "./*.md".into(),
        "".into(),
    )
    .unwrap();
    assert_eq!(
        r.hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
        vec!["README.md"]
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_project_text_rejects_invalid_glob() {
    let dir = temp_project_dir("search-badglob");
    assert!(matches!(
        search_project_text(
            dir.clone(),
            "needle".into(),
            false,
            false,
            false,
            "src/[".into(),
            "".into(),
        ),
        Err(ref e) if e.is_code(ErrorCode::SearchInvalidGlob)
    ));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_project_text_invalid_regex() {
    let dir = temp_project_dir("search-badre");
    assert!(matches!(
        search_project_text(dir.clone(), "([".into(), false, false, true, "".into(), "".into()),
        Err(ref e) if e.is_code(ErrorCode::SearchInvalidRegex)
    ));
    let _ = fs::remove_dir_all(&dir);
}
