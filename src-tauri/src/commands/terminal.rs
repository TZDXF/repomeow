//! 项目内嵌终端:在项目目录内直接执行命令(npm scripts / docker compose / 自定义命令),
//! 子进程管道捕获 stdout/stderr,经 `terminal://output` 事件流式推送前端(xterm 渲染),
//! 支持实时输出查看、stdin 写入、停止(Windows 整棵树 taskkill)与重启。
//! 与「系统终端新窗口」模式(run_in_terminal)并存,由设置项 embeddedTerminal 分流。
//!
//! 会话仅存内存:应用退出即清空,不做持久化。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tauri::{AppHandle, Emitter, State};

use crate::commands::open::{hidden, resolve_shell, ShellKind};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{TerminalSessionInfo, TerminalSessionStatus};
use crate::time_util::now_ts;

/// 会话元数据变更(启动/退出/停止/重启),payload 为 TerminalSessionInfo
pub const SESSION_CHANGED_EVENT: &str = "terminal://session-changed";
/// 会话输出增量,payload 为 { id, data }
pub const OUTPUT_EVENT: &str = "terminal://output";

/// 单会话输出回放缓冲上限(字符数):超出丢弃最旧部分
const MAX_OUTPUT_CHARS: usize = 400_000;
/// 会话总数上限:超出时淘汰最旧的已结束会话(运行中的不淘汰)
const MAX_SESSIONS: usize = 64;

/// 一次执行的可变参数(重启时从既有会话还原)
struct SessionSpec {
    command: String,
    cwd: String,
    java_home: Option<String>,
    interactive: bool,
}

struct SessionEntry {
    info: TerminalSessionInfo,
    /// 递增代数:重启后旧 reader/waiter 线程的迟到回调按代数不符丢弃,
    /// 避免旧进程退出事件把新会话误标为 stopped
    generation: u64,
    pid: u32,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    /// 重启所需的完整参数(java_home 不入 info,只在后端留存)
    spec: SessionSpec,
    /// 用户主动停止标记:waiter 据此把退出归类为 stopped 而非 exited
    stop_requested: bool,
    output: VecDeque<String>,
    output_chars: usize,
}

/// 内嵌终端会话注册表(lib.rs setup 中 manage,命令经 State<Arc<TerminalManager>> 取)
#[derive(Default)]
pub struct TerminalManager {
    sessions: Mutex<HashMap<u64, SessionEntry>>,
    next_id: AtomicU64,
}

impl TerminalManager {
    fn lock(&self) -> MutexGuard<'_, HashMap<u64, SessionEntry>> {
        self.sessions.lock().unwrap()
    }

    /// 淘汰最旧的已结束会话,控制内存占用;运行中的会话永不淘汰
    fn prune(&self) {
        let mut sessions = self.lock();
        if sessions.len() <= MAX_SESSIONS {
            return;
        }
        let mut finished: Vec<(u64, i64)> = sessions
            .iter()
            .filter(|(_, e)| e.info.status != TerminalSessionStatus::Running)
            .map(|(id, e)| (*id, e.info.started_at))
            .collect();
        finished.sort_by_key(|(_, started)| *started);
        let excess = sessions.len() - MAX_SESSIONS;
        for (id, _) in finished.into_iter().take(excess) {
            sessions.remove(&id);
        }
    }
}

fn session_not_found(id: u64) -> AppError {
    AppError::coded(ErrorCode::TerminalSessionNotFound, id.to_string())
}

fn push_output(entry: &mut SessionEntry, chunk: String) {
    entry.output_chars += chunk.len();
    entry.output.push_back(chunk);
    while entry.output_chars > MAX_OUTPUT_CHARS {
        match entry.output.pop_front() {
            Some(front) => entry.output_chars -= front.len(),
            None => {
                entry.output_chars = 0;
                break;
            }
        }
    }
}

/// GBK 首/次字节范围(中文 Windows 下 cmd 内建命令与部分工具的原生输出编码)
fn is_gbk_lead(b: u8) -> bool {
    (0x81..=0xFE).contains(&b)
}
fn is_gbk_trail(b: u8) -> bool {
    (0x40..=0xFE).contains(&b) && b != 0x7F
}

/// 流式字节解码:优先 UTF-8(npm/docker 等现代工具);非法序列尝试按 GBK
/// 双字节消费(中文 Windows 的系统级错误文案);仍不匹配用替换字符逐字节跳过。
/// 尾部不完整的多字节序列(两种编码都可能被 read 切半)留存等待下一块。
#[derive(Default)]
struct StreamDecoder {
    carry: Vec<u8>,
}

impl StreamDecoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.carry.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.carry) {
                Ok(s) => {
                    out.push_str(s);
                    self.carry.clear();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    // valid 之前必为合法 UTF-8(from_utf8 已校验)
                    out.push_str(std::str::from_utf8(&self.carry[..valid]).unwrap());
                    self.carry.drain(..valid);
                    match e.error_len() {
                        // 尾部不完整的多字节序列,等下一次 push
                        None => break,
                        Some(_) => {
                            if self.carry.len() >= 2
                                && is_gbk_lead(self.carry[0])
                                && is_gbk_trail(self.carry[1])
                            {
                                let (s, _, _) = encoding_rs::GBK.decode(&self.carry[..2]);
                                out.push_str(&s);
                                self.carry.drain(..2);
                            } else if self.carry.len() == 1 && is_gbk_lead(self.carry[0]) {
                                break; // GBK 第二字节尚未到达
                            } else {
                                out.push('\u{FFFD}');
                                self.carry.drain(..1);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// EOF 冲刷:残留字节按 UTF-8 有损解码(不完整序列落替换字符)
    fn flush(&mut self) -> String {
        if self.carry.is_empty() {
            return String::new();
        }
        let out = String::from_utf8_lossy(&self.carry).into_owned();
        self.carry.clear();
        out
    }
}

/// 按设置的 shell 构造内嵌执行命令(管道模式,无窗口)。
/// 与 spawn_terminal 的窗口模式共用 shell 选择与可用性回退。
fn build_shell_command(app: &AppHandle, command: &str) -> Command {
    let shell = resolve_shell(app);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        match shell {
            ShellKind::Cmd => {
                let mut c = hidden(Command::new("cmd"));
                // raw_arg:命令原样拼接,等价于在 cmd 里逐字输入(不加额外引号);
                // 多行先摊平(cmd /C 遇换行即截断)
                let flat = crate::commands::open::flatten_multiline(Some(command), shell.separator());
                c.raw_arg(format!("/C {}", flat.as_deref().unwrap_or(command)));
                c
            }
            ShellKind::PowerShell => {
                let ps = crate::commands::open::find_powershell().unwrap_or("powershell");
                // EncodedCommand 规避命令文本在外层进程与 powershell 之间的引号转义;
                // 前缀把管道输出编码切到 UTF-8,减少中文输出按 GBK 写出的场景
                let script =
                    format!("[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; {command}");
                let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(utf16);
                let mut c = hidden(Command::new(ps));
                c.raw_arg(format!("-NoProfile -EncodedCommand {b64}"));
                c
            }
            ShellKind::GitBash => {
                let bash =
                    crate::commands::open::find_git_bash().unwrap_or_else(|| "bash".into());
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(command.as_bytes());
                // base64 负载不经 bash 分词,命令中的引号/特殊字符原样保留;
                // source 的退出码即命令退出码(与窗口模式的 exec bash 保活不同,这里跑完即退)
                let mut c = hidden(Command::new(bash));
                c.arg("-c").arg(format!("source <(echo {b64} | base64 -d)"));
                c
            }
        }
    }
    #[cfg(not(windows))]
    {
        // 非 Windows 无 shell 选择(resolve_shell 恒为 Cmd),sh -c 原生支持多行脚本
        let _ = shell;
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// 手动新建的会话直接运行交互式 shell,而非执行完一次命令就退出。
/// 仍通过 stdin/stdout 管道通信(非 PTY),适合逐行输入命令。
fn build_interactive_shell(app: &AppHandle) -> Command {
    let shell = resolve_shell(app);
    #[cfg(windows)]
    {
        match shell {
            ShellKind::Cmd => {
                let mut c = hidden(Command::new("cmd"));
                c.args(["/Q", "/K"]);
                c
            }
            ShellKind::PowerShell => {
                let ps = crate::commands::open::find_powershell().unwrap_or("powershell");
                let mut c = hidden(Command::new(ps));
                c.args(["-NoLogo", "-NoProfile", "-NoExit", "-Command", "-"]);
                c
            }
            ShellKind::GitBash => {
                let bash = crate::commands::open::find_git_bash().unwrap_or_else(|| "bash".into());
                let mut c = hidden(Command::new(bash));
                c.arg("-i");
                c
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = shell;
        let mut c = Command::new("sh");
        c.arg("-i");
        c
    }
}

fn spawn_child(app: &AppHandle, spec: &SessionSpec) -> AppResult<Child> {
    let mut cmd = if spec.interactive {
        build_interactive_shell(app)
    } else {
        build_shell_command(app, &spec.command)
    };
    cmd.current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 管道模式下 npm(chalk)等检测到非 TTY 默认关颜色,强制开启
        // (xterm 前端能渲染 ANSI 序列;不支持的工具会自行忽略这些变量)
        .env("FORCE_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("TERM", "xterm-256color");
    // JAVA_HOME 用进程环境注入(窗口模式因跨 wt/start 只能拼命令前缀,管道模式无此限制)
    if let Some(home) = spec.java_home.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cmd.env("JAVA_HOME", home.replace('"', ""));
    }
    cmd.spawn()
        .map_err(|e| AppError::coded(ErrorCode::TerminalSpawnFailed, e.to_string()))
}

/// 结束进程树:Windows 用 taskkill /T 连带子进程(npm 脚本常派生 node 子进程,
/// 只杀 shell 会留下孤儿);其他平台退回 kill 主进程
fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = hidden(Command::new("taskkill"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
    }
}

fn spawn_reader(
    app: AppHandle,
    mgr: Arc<TerminalManager>,
    id: u64,
    generation: u64,
    mut reader: impl Read + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut decoder = StreamDecoder::default();
        let mut buf = [0u8; 8192];
        let push_chunk = |chunk: String| -> bool {
            if chunk.is_empty() {
                return true;
            }
            {
                let mut sessions = mgr.lock();
                let Some(entry) = sessions.get_mut(&id) else {
                    return false;
                };
                // 重启后的旧线程迟到输出:丢弃,避免污染新会话缓冲
                if entry.generation != generation {
                    return false;
                }
                push_output(entry, chunk.clone());
            }
            let _ = app.emit(OUTPUT_EVENT, serde_json::json!({ "id": id, "data": chunk }));
            true
        };
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if !push_chunk(decoder.push(&buf[..n])) {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        push_chunk(decoder.flush());
    });
}

fn spawn_waiter(app: AppHandle, mgr: Arc<TerminalManager>, id: u64, generation: u64, mut child: Child) {
    std::thread::spawn(move || {
        let status = child.wait();
        let info = {
            let mut sessions = mgr.lock();
            let Some(entry) = sessions.get_mut(&id) else {
                return; // 会话已被移除
            };
            if entry.generation != generation {
                return; // 旧进程退出(restart 后),状态已由新代数接管
            }
            entry.stdin = None;
            entry.info.finished_at = Some(now_ts());
            if entry.stop_requested {
                entry.info.status = TerminalSessionStatus::Stopped;
            } else {
                entry.info.exit_code = status.ok().and_then(|s| s.code());
                entry.info.status = TerminalSessionStatus::Exited;
            }
            entry.info.clone()
        };
        let _ = app.emit(SESSION_CHANGED_EVENT, &info);
    });
}

/// 登记新会话并接管子进程管道(reader/waiter 线程 + 事件广播)
fn register_launch(
    app: &AppHandle,
    mgr: &Arc<TerminalManager>,
    generation: u64,
    info: TerminalSessionInfo,
    spec: SessionSpec,
    mut child: Child,
) -> TerminalSessionInfo {
    let id = info.id;
    let stdin = child.stdin.take().map(|s| Arc::new(Mutex::new(s)));
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let pid = child.id();
    {
        let mut sessions = mgr.lock();
        sessions.insert(
            id,
            SessionEntry {
                info: info.clone(),
                generation,
                pid,
                stdin,
                spec,
                stop_requested: false,
                output: VecDeque::new(),
                output_chars: 0,
            },
        );
    }
    if let Some(out) = stdout {
        spawn_reader(app.clone(), mgr.clone(), id, generation, out);
    }
    if let Some(err) = stderr {
        spawn_reader(app.clone(), mgr.clone(), id, generation, err);
    }
    spawn_waiter(app.clone(), mgr.clone(), id, generation, child);
    let _ = app.emit(SESSION_CHANGED_EVENT, &info);
    info
}

/// 在项目内嵌终端执行命令(默认模式;设置关闭时前端走 run_in_terminal 系统终端)。
/// cwd 缺省为项目根 path(monorepo 子包内执行 npm run 时传子目录);
/// java_home 非空时以进程环境注入 JAVA_HOME;kind/label 仅用于前端分组展示。
#[tauri::command]
pub fn run_command_session(
    app: AppHandle,
    manager: State<'_, Arc<TerminalManager>>,
    project_id: i64,
    project_name: String,
    path: String,
    command: String,
    cwd: Option<String>,
    java_home: Option<String>,
    kind: Option<String>,
    label: Option<String>,
) -> AppResult<TerminalSessionInfo> {
    let work_dir = cwd.unwrap_or(path);
    if !std::path::Path::new(&work_dir).is_dir() {
        return Err(AppError::coded(ErrorCode::ScriptDirNotFound, work_dir));
    }
    let mgr = manager.inner().clone();
    let id = mgr.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let spec = SessionSpec {
        command,
        cwd: work_dir,
        java_home,
        interactive: false,
    };
    let child = spawn_child(&app, &spec)?;
    let info = TerminalSessionInfo {
        id,
        project_id,
        project_name,
        label: label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| spec.command.clone()),
        kind: kind.unwrap_or_else(|| "shell".into()),
        command: spec.command.clone(),
        interactive: false,
        cwd: spec.cwd.clone(),
        status: TerminalSessionStatus::Running,
        exit_code: None,
        started_at: now_ts(),
        finished_at: None,
    };
    let info = register_launch(&app, &mgr, 0, info, spec, child);
    mgr.prune();
    Ok(info)
}

/// 在当前项目(或选中的 worktree)目录手动打开可输入命令的 shell 会话。
#[tauri::command]
pub fn create_shell_session(
    app: AppHandle,
    manager: State<'_, Arc<TerminalManager>>,
    project_id: i64,
    project_name: String,
    path: String,
) -> AppResult<TerminalSessionInfo> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(AppError::coded(ErrorCode::ScriptDirNotFound, path));
    }
    let mgr = manager.inner().clone();
    let id = mgr.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let spec = SessionSpec {
        command: String::new(),
        cwd: path,
        java_home: None,
        interactive: true,
    };
    let child = spawn_child(&app, &spec)?;
    let info = TerminalSessionInfo {
        id,
        project_id,
        project_name,
        label: "Shell".into(),
        kind: "shell".into(),
        command: "Shell".into(),
        interactive: true,
        cwd: spec.cwd.clone(),
        status: TerminalSessionStatus::Running,
        exit_code: None,
        started_at: now_ts(),
        finished_at: None,
    };
    let info = register_launch(&app, &mgr, 0, info, spec, child);
    mgr.prune();
    Ok(info)
}

/// 全部会话:运行中优先,其余按启动时间倒序
#[tauri::command]
pub fn list_command_sessions(manager: State<'_, Arc<TerminalManager>>) -> Vec<TerminalSessionInfo> {
    let sessions = manager.lock();
    let mut list: Vec<TerminalSessionInfo> = sessions.values().map(|e| e.info.clone()).collect();
    list.sort_by(|a, b| {
        (b.status == TerminalSessionStatus::Running)
            .cmp(&(a.status == TerminalSessionStatus::Running))
            .then(b.started_at.cmp(&a.started_at))
    });
    list
}

/// 回放会话输出缓冲(前端打开详情页时一次性拉取,此后走事件增量)
#[tauri::command]
pub fn get_command_session_output(
    manager: State<'_, Arc<TerminalManager>>,
    id: u64,
) -> AppResult<String> {
    let sessions = manager.lock();
    let entry = sessions.get(&id).ok_or_else(|| session_not_found(id))?;
    Ok(entry.output.iter().map(String::as_str).collect())
}

/// 停止会话(整棵树);进程退出后 waiter 把状态归为 stopped
#[tauri::command]
pub fn stop_command_session(manager: State<'_, Arc<TerminalManager>>, id: u64) -> AppResult<()> {
    let mut sessions = manager.lock();
    let entry = sessions.get_mut(&id).ok_or_else(|| session_not_found(id))?;
    if entry.info.status == TerminalSessionStatus::Running {
        entry.stop_requested = true;
        kill_tree(entry.pid);
    }
    Ok(())
}

/// 重启会话:运行中先整棵树停止,再以原参数重新拉起;会话 id 不变,
/// 输出缓冲清空,代数递增使旧线程迟到事件失效
#[tauri::command]
pub fn restart_command_session(
    app: AppHandle,
    manager: State<'_, Arc<TerminalManager>>,
    id: u64,
) -> AppResult<TerminalSessionInfo> {
    let mgr = manager.inner().clone();
    let spec = {
        let mut sessions = mgr.lock();
        let entry = sessions.get_mut(&id).ok_or_else(|| session_not_found(id))?;
        if entry.info.status == TerminalSessionStatus::Running {
            entry.stop_requested = true;
            kill_tree(entry.pid);
        }
        SessionSpec {
            command: entry.spec.command.clone(),
            cwd: entry.spec.cwd.clone(),
            java_home: entry.spec.java_home.clone(),
            interactive: entry.spec.interactive,
        }
    };
    let mut child = match spawn_child(&app, &spec) {
        Ok(c) => c,
        Err(e) => {
            // 重启拉起失败:会话标记为启动失败,保留在列表里供用户查看/重试
            let info = {
                let mut sessions = mgr.lock();
                let entry = sessions.get_mut(&id).ok_or_else(|| session_not_found(id))?;
                entry.info.status = TerminalSessionStatus::SpawnFailed;
                entry.info.finished_at = Some(now_ts());
                entry.info.clone()
            };
            let _ = app.emit(SESSION_CHANGED_EVENT, &info);
            return Err(e);
        }
    };
    let stdin = child.stdin.take().map(|s| Arc::new(Mutex::new(s)));
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let pid = child.id();
    let (info, generation) = {
        let mut sessions = mgr.lock();
        let entry = sessions.get_mut(&id).ok_or_else(|| session_not_found(id))?;
        entry.generation += 1;
        entry.pid = pid;
        entry.stdin = stdin;
        entry.stop_requested = false;
        entry.output.clear();
        entry.output_chars = 0;
        entry.info.status = TerminalSessionStatus::Running;
        entry.info.exit_code = None;
        entry.info.started_at = now_ts();
        entry.info.finished_at = None;
        (entry.info.clone(), entry.generation)
    };
    if let Some(out) = stdout {
        spawn_reader(app.clone(), mgr.clone(), id, generation, out);
    }
    if let Some(err) = stderr {
        spawn_reader(app.clone(), mgr.clone(), id, generation, err);
    }
    spawn_waiter(app.clone(), mgr.clone(), id, generation, child);
    let _ = app.emit(SESSION_CHANGED_EVENT, &info);
    Ok(info)
}

/// 向会话 stdin 写入(xterm 键盘输入);会话已结束时静默忽略
#[tauri::command]
pub fn write_command_session(
    manager: State<'_, Arc<TerminalManager>>,
    id: u64,
    data: String,
) -> AppResult<()> {
    let (stdin, interactive) = {
        let sessions = manager.lock();
        let entry = sessions.get(&id).ok_or_else(|| session_not_found(id))?;
        (entry.stdin.clone(), entry.spec.interactive)
    };
    if let Some(stdin) = stdin {
        let mut guard = stdin.lock().unwrap();
        // 子进程已退出但 waiter 尚未清理 stdin 时写入会 EPIPE,按静默处理
        let input = if interactive { data.replace('\r', "\n") } else { data };
        let _ = guard.write_all(input.as_bytes());
        let _ = guard.flush();
    }
    Ok(())
}

/// 从注册表移除会话(运行中的先停止);前端「清除已结束」逐条调用
#[tauri::command]
pub fn remove_command_session(manager: State<'_, Arc<TerminalManager>>, id: u64) -> AppResult<()> {
    let mut sessions = manager.lock();
    let Some(entry) = sessions.get_mut(&id) else {
        return Err(session_not_found(id));
    };
    if entry.info.status == TerminalSessionStatus::Running {
        entry.stop_requested = true;
        kill_tree(entry.pid);
    }
    sessions.remove(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_utf8_split_multibyte() {
        let mut d = StreamDecoder::default();
        // "中" 的 UTF-8 是 E4 B8 AD,切成两块推送
        let first = d.push(&[0xE4, 0xB8]);
        assert_eq!(first, "", "不完整序列应留存等待");
        let second = d.push(&[0xAD]);
        assert_eq!(second, "中");
    }

    #[test]
    fn decoder_gbk_fallback() {
        let mut d = StreamDecoder::default();
        // "中文" 的 GBK 编码:D6 D0 C4 C4
        assert_eq!(d.push(&[0xD6, 0xD0, 0xC4, 0xC4]), "中文");
    }

    #[test]
    fn decoder_gbk_split_across_reads() {
        let mut d = StreamDecoder::default();
        assert_eq!(d.push(&[0xD6]), "", "GBK 首字节应等待第二字节");
        assert_eq!(d.push(&[0xD0]), "中");
    }

    #[test]
    fn decoder_mixed_ascii_gbk_and_invalid() {
        let mut d = StreamDecoder::default();
        // "ok: " + GBK "中" + 非法字节 0xFF
        assert_eq!(d.push(b"ok: \xD6\xD0\xFF"), "ok: 中\u{FFFD}");
    }

    #[test]
    fn decoder_utf8_content_not_misread_as_gbk() {
        let mut d = StreamDecoder::default();
        let s = "vite v5.0 就绪 ✓";
        assert_eq!(d.push(s.as_bytes()), s);
    }

    #[test]
    fn output_buffer_capped() {
        let mut entry = SessionEntry {
            info: TerminalSessionInfo {
                id: 1,
                project_id: 1,
                project_name: "p".into(),
                label: "l".into(),
                kind: "shell".into(),
                command: "c".into(),
                interactive: false,
                cwd: ".".into(),
                status: TerminalSessionStatus::Running,
                exit_code: None,
                started_at: 0,
                finished_at: None,
            },
            generation: 0,
            pid: 0,
            stdin: None,
            spec: SessionSpec {
                command: "c".into(),
                cwd: ".".into(),
                java_home: None,
                interactive: false,
            },
            stop_requested: false,
            output: VecDeque::new(),
            output_chars: 0,
        };
        for _ in 0..300 {
            push_output(&mut entry, "x".repeat(2000));
        }
        assert!(entry.output_chars <= MAX_OUTPUT_CHARS);
        assert!(!entry.output.is_empty());
    }
}
