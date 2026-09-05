//! 资源库加密:Argon2id 口令派生 + XChaCha20Poly1305 文件容器。
//!
//! 口令只在内存中出现:解锁后派生密钥存于进程内注册表(按库根路径分片),
//! `lock`/进程退出即零化;磁盘上只有密文 + 明文 meta 里的盐与口令校验值。
//!
//! 容器格式: `RLENC1`(6B) | version u8(1B) | XNonce(24B) | 密文(含 16B tag)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use super::errors::{codes, RlError, RlResult};
use super::models::KeyCheck;

/// 加密容器魔数
pub const CONTAINER_MAGIC: &[u8; 6] = b"RLENC1";
const CONTAINER_VERSION: u8 = 1;
/// XChaCha20 扩展 nonce 长度
const XNONCE_LEN: usize = 24;
/// 派生密钥长度(256 bit)
const KEY_LEN: usize = 32;
/// 盐长度(argon2 推荐 16 字节)
const SALT_LEN: usize = 16;
/// 口令校验固定明文:解锁时先验口令,把「口令错误」与「文件损坏」区分开
const KEY_CHECK_PLAINTEXT: &[u8] = b"RepoMeow resource library key check v1";

/// 生产 KDF 参数(OWASP 推荐下限:Argon2id, m=19456 KiB, t=2, p=1)
fn prod_params() -> Params {
    Params::new(19456, 2, 1, Some(KEY_LEN)).expect("固定 KDF 参数恒合法")
}

/// 测试用轻量参数(仅单测直接构造时使用;生产解锁恒走 prod_params)
#[cfg(test)]
fn test_params() -> Params {
    Params::new(64, 1, 1, Some(KEY_LEN)).expect("测试 KDF 参数恒合法")
}

fn derive(password: &[u8], salt_b64: &str, params: &Params) -> RlResult<Zeroizing<[u8; KEY_LEN]>> {
    let salt = B64
        .decode(salt_b64)
        .map_err(|e| RlError::corrupt("kdf salt 解码", e))?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone())
        .hash_password_into(password, &salt, key.as_mut())
        .map_err(|e| RlError::corrupt("argon2id 派生失败", e))?;
    Ok(key)
}

/// 生成随机盐(base64,16 字节,供 meta.kdf_salt 持久化)
pub fn new_salt() -> String {
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).expect("系统随机源不可用");
    B64.encode(salt)
}

/// 由口令 + 已存盐派生 256bit 密钥(生产参数)
pub fn derive_key(password: &str, salt_b64: &str) -> RlResult<Zeroizing<[u8; KEY_LEN]>> {
    derive(password.as_bytes(), salt_b64, &prod_params())
}

fn new_nonce() -> [u8; XNONCE_LEN] {
    let mut nonce = [0u8; XNONCE_LEN];
    getrandom::fill(&mut nonce).expect("系统随机源不可用");
    nonce
}

fn cipher(key: &[u8; KEY_LEN]) -> XChaCha20Poly1305 {
    let k: Key = Key::from(*key);
    XChaCha20Poly1305::new(&k)
}

fn encrypt_raw(
    key: &[u8; KEY_LEN],
    nonce: &[u8; XNONCE_LEN],
    plaintext: &[u8],
) -> RlResult<Vec<u8>> {
    use chacha20poly1305::aead::Aead;
    let n: XNonce = XNonce::from(*nonce);
    cipher(key)
        .encrypt(&n, plaintext)
        .map_err(|_| RlError::coded(codes::CORRUPT, "aead 加密失败"))
}

fn decrypt_raw(
    key: &[u8; KEY_LEN],
    nonce: &[u8; XNONCE_LEN],
    ciphertext: &[u8],
) -> RlResult<Zeroizing<Vec<u8>>> {
    use chacha20poly1305::aead::Aead;
    let n: XNonce = XNonce::from(*nonce);
    cipher(key)
        .decrypt(&n, ciphertext)
        .map(Zeroizing::new)
        .map_err(|_| RlError::coded(codes::CORRUPT, "aead 校验失败"))
}

/// 生成口令校验值(随机 nonce + 固定明文的密文,base64)
pub fn make_key_check(key: &[u8; KEY_LEN]) -> RlResult<KeyCheck> {
    let nonce = new_nonce();
    let ciphertext = encrypt_raw(key, &nonce, KEY_CHECK_PLAINTEXT)?;
    Ok(KeyCheck {
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    })
}

/// 校验口令派生出的密钥(解锁时区分口令错误/数据损坏)
pub fn verify_key_check(key: &[u8; KEY_LEN], check: &KeyCheck) -> bool {
    let (Ok(nonce), Ok(ciphertext)) = (B64.decode(&check.nonce), B64.decode(&check.ciphertext))
    else {
        return false;
    };
    let Ok(nonce) = <[u8; XNONCE_LEN]>::try_from(nonce.as_slice()) else {
        return false;
    };
    matches!(decrypt_raw(key, &nonce, &ciphertext), Ok(plain) if plain.as_slice() == &KEY_CHECK_PLAINTEXT[..])
}

/// 加密为磁盘容器(每次调用使用全新随机 nonce)
pub fn encrypt_bytes(key: &[u8; KEY_LEN], plaintext: &[u8]) -> RlResult<Vec<u8>> {
    let nonce = new_nonce();
    let body = encrypt_raw(key, &nonce, plaintext)?;
    let mut out = Vec::with_capacity(CONTAINER_MAGIC.len() + 1 + XNONCE_LEN + body.len());
    out.extend_from_slice(CONTAINER_MAGIC);
    out.push(CONTAINER_VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    Ok(out)
}

/// 是否为加密容器字节
pub fn is_container(data: &[u8]) -> bool {
    data.starts_with(CONTAINER_MAGIC)
}

/// 解密磁盘字节。`key` 为 None 时,非容器字节(明文残留)直通容错——
/// 覆盖「enable 写到一半崩溃」等过渡态;容器字节则要求已解锁。
pub fn decrypt_bytes(key: Option<&[u8; KEY_LEN]>, data: &[u8]) -> RlResult<Zeroizing<Vec<u8>>> {
    if !is_container(data) {
        return Ok(Zeroizing::new(data.to_vec()));
    }
    if data.len() < CONTAINER_MAGIC.len() + 1 + XNONCE_LEN + 16 {
        return Err(RlError::coded(codes::CORRUPT, "加密容器长度不足"));
    }
    if data[CONTAINER_MAGIC.len()] != CONTAINER_VERSION {
        return Err(RlError::coded(
            codes::CORRUPT,
            format!("未知容器版本 {}", data[CONTAINER_MAGIC.len()]),
        ));
    }
    let key = key.ok_or_else(|| RlError::coded(codes::LOCKED, ""))?;
    let nonce: [u8; XNONCE_LEN] = data
        [CONTAINER_MAGIC.len() + 1..CONTAINER_MAGIC.len() + 1 + XNONCE_LEN]
        .try_into()
        .expect("nonce 长度已校验");
    let ciphertext = &data[CONTAINER_MAGIC.len() + 1 + XNONCE_LEN..];
    decrypt_raw(key, &nonce, ciphertext)
}

// ── 进程内密钥注册表(口令仅内存)───────────────────────────────────────

static KEYS: LazyLock<Mutex<HashMap<PathBuf, Zeroizing<[u8; KEY_LEN]>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 解锁后把派生密钥登记进内存(按库根路径分片,多库互不干扰)
pub fn store_key(root: &Path, key: Zeroizing<[u8; KEY_LEN]>) {
    KEYS.lock()
        .expect("密钥表锁中毒")
        .insert(root.to_path_buf(), key);
}

/// lock / 禁用加密 / 远端导入后清除内存密钥
pub fn clear_key(root: &Path) {
    KEYS.lock().expect("密钥表锁中毒").remove(root);
}

/// 取当前进程内已解锁的密钥(未解锁返回 None)
pub fn key_for(root: &Path) -> Option<Zeroizing<[u8; KEY_LEN]>> {
    KEYS.lock().expect("密钥表锁中毒").get(root).cloned()
}

pub fn is_unlocked(root: &Path) -> bool {
    KEYS.lock().expect("密钥表锁中毒").contains_key(root)
}

#[cfg(test)]
pub(crate) fn derive_key_test(
    password: &str,
    salt_b64: &str,
) -> RlResult<Zeroizing<[u8; KEY_LEN]>> {
    derive(password.as_bytes(), salt_b64, &test_params())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> Zeroizing<[u8; KEY_LEN]> {
        let mut k = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(k.as_mut()).unwrap();
        k
    }

    #[test]
    fn roundtrip_preserves_plaintext() {
        let key = test_key();
        let container = encrypt_bytes(&key, "你好,喵库".as_bytes()).unwrap();
        assert!(is_container(&container));
        let out = decrypt_bytes(Some(&key), &container).unwrap();
        assert_eq!(out.as_slice(), "你好,喵库".as_bytes());
    }

    #[test]
    fn nonce_is_fresh_each_encrypt() {
        let key = test_key();
        let a = encrypt_bytes(&key, b"same").unwrap();
        let b = encrypt_bytes(&key, b"same").unwrap();
        assert_ne!(&a[7..31], &b[7..31], "两次加密的 nonce 必须不同");
    }

    #[test]
    fn tampered_ciphertext_fails_aead() {
        let key = test_key();
        let mut container = encrypt_bytes(&key, b"secret").unwrap();
        let last = container.len() - 1;
        container[last] ^= 0x01;
        assert!(decrypt_bytes(Some(&key), &container).is_err());
    }

    #[test]
    fn wrong_key_fails_aead() {
        let key = test_key();
        let other = test_key();
        let container = encrypt_bytes(&key, b"secret").unwrap();
        assert!(decrypt_bytes(Some(&other), &container).is_err());
    }

    #[test]
    fn plaintext_passthrough_tolerates_transition_state() {
        let out = decrypt_bytes(None, b"{\"plain\":true}").unwrap();
        assert_eq!(out.as_slice(), b"{\"plain\":true}");
    }

    #[test]
    fn container_requires_unlock() {
        let key = test_key();
        let container = encrypt_bytes(&key, b"x").unwrap();
        let err = decrypt_bytes(None, &container).unwrap_err();
        assert_eq!(err.code(), codes::LOCKED);
    }

    #[test]
    fn key_check_verifies_correct_password() {
        let key = test_key();
        let check = make_key_check(&key).unwrap();
        assert!(verify_key_check(&key, &check));
        let other = test_key();
        assert!(!verify_key_check(&other, &check));
    }

    #[test]
    fn derive_matches_same_salt_and_password() {
        let salt = new_salt();
        let a = derive_key_test("p@ssw0rd", &salt).unwrap();
        let b = derive_key_test("p@ssw0rd", &salt).unwrap();
        assert_eq!(a.as_ref(), b.as_ref());
        let c = derive_key_test("p@ssw0rd!", &salt).unwrap();
        assert_ne!(a.as_ref(), c.as_ref());
    }

    #[test]
    fn registry_scopes_by_root() {
        let root_a = Path::new("/tmp/a");
        let root_b = Path::new("/tmp/b");
        clear_key(root_a);
        clear_key(root_b);
        assert!(!is_unlocked(root_a));
        store_key(root_a, test_key());
        assert!(is_unlocked(root_a));
        assert!(!is_unlocked(root_b));
        clear_key(root_a);
        assert!(!is_unlocked(root_a));
    }
}
