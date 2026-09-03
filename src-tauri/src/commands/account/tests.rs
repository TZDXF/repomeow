use super::gh::*;
use super::http::*;
use super::*;

use super::*;

#[test]
fn token_preview_masks_all_but_last4() {
    assert_eq!(token_preview("ghp_abcdef123456"), "****3456");
    assert_eq!(token_preview("abc"), "****");
}

#[test]
fn resolve_base_url_rules() {
    assert_eq!(
        resolve_base_url("github", None).unwrap(),
        "https://github.com"
    );
    assert_eq!(
        resolve_base_url("gitee", None).unwrap(),
        "https://gitee.com"
    );
    // gitlab: 去尾斜杠,允许 http 内网地址
    assert_eq!(
        resolve_base_url("gitlab", Some("https://gitlab.example.com/")).unwrap(),
        "https://gitlab.example.com"
    );
    assert_eq!(
        resolve_base_url("gitlab", Some("http://192.168.1.10:8080")).unwrap(),
        "http://192.168.1.10:8080"
    );
    assert!(resolve_base_url("gitlab", Some("")).is_err());
    assert!(resolve_base_url("gitlab", Some("gitlab.example.com")).is_err());
}

#[test]
fn build_authed_url_embeds_credentials() {
    assert_eq!(
        build_authed_url("github", "octo", "tok", "https://github.com/a/b.git"),
        "https://x-access-token:tok@github.com/a/b.git"
    );
    assert_eq!(
        build_authed_url("gitlab", "u", "tok", "https://lab.local/a/b.git"),
        "https://oauth2:tok@lab.local/a/b.git"
    );
    assert_eq!(
        build_authed_url("gitee", "octo", "tok", "https://gitee.com/a/b.git"),
        "https://octo:tok@gitee.com/a/b.git"
    );
    // ssh 地址不处理
    assert_eq!(
        build_authed_url("github", "octo", "tok", "git@github.com:a/b.git"),
        "git@github.com:a/b.git"
    );
}

#[test]
fn db_roundtrip() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    let ts = now();
    conn.execute(
        "INSERT INTO git_accounts (provider, label, base_url, username, token, created_at, updated_at)
         VALUES ('github', '工作', 'https://github.com', 'octo', 'ghp_secret1234', ?1, ?1)",
        params![ts],
    )
    .unwrap();
    let row = get_account_row(&conn, 1).unwrap();
    assert_eq!(row.username, "octo");
    let account = row_to_account(&row);
    assert_eq!(account.token_preview, "****1234");
    assert_eq!(account.provider, "github");
    // 迁移新增列默认未失效
    assert!(!account.token_invalid);

    let (provider, username, token) = get_credentials(&conn, 1).unwrap();
    assert_eq!(
        (provider.as_str(), username.as_str(), token.as_str()),
        ("github", "octo", "ghp_secret1234")
    );

    assert!(get_account_row(&conn, 999).is_err());
}
