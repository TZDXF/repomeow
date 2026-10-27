//! 项目内嵌终端:在项目目录内直接执行命令(npm scripts / docker compose / 自定义命令),
//! 输出经 `terminal://output` 事件流式推送前端(xterm 渲染),
//! 支持实时输出查看、stdin 写入、停止(Windows 整棵树 taskkill)与重启。
//! npm scripts / docker compose / 自定义命令等统一在应用内终端执行。
//!
//! 两种会话形态:
//! - 命令会话:在 PTY 内执行命令,结束后退出,支持 CLI 快捷键与交互提示;
//! - 常驻 Shell:在 PTY 内持续接收命令,提示符/回显/行编辑由 shell 自绘。
//!
//! 会话仅存内存:应用退出即清空,不做持久化。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use tauri::{AppHandle, Emitter, State};

use crate::commands::open::{hidden, resolve_shell, resolve_shell_choice, ShellKind};
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
    /// 是否为手动新建的常驻 Shell;命令会话同样使用 PTY。
    interactive: bool,
    /// 手动新建终端时前端显式选择的 shell;None 表示按设置项解析(命令会话)
    shell: Option<ShellKind>,
}

struct SessionEntry {
    info: TerminalSessionInfo,
    /// 递增代数:重启后旧 reader/waiter 线程的迟到回调按代数不符丢弃,
    /// 避免旧进程退出事件把新会话误标为 stopped
    generation: u64,
    pid: u32,
    /// PTY 输入端(命令与常驻 Shell 共用)
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    /// PTY 主端(resize 用);进程退出后关闭以释放 ConPTY 句柄
    master: Option<Box<dyn MasterPty + Send>>,
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

/// 命令与常驻 Shell 均使用 PTY 子进程,共用等待退出与进程树管理逻辑。
type SessionChild = Box<dyn portable_pty::Child + Send + Sync>;

/// 一次拉起的全部句柄:子进程 + PTY 输出流 + 输入端 + PTY 主端
struct Launched {
    child: SessionChild,
    readers: Vec<Box<dyn Read + Send>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    master: Option<Box<dyn MasterPty + Send>>,
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

/// 构造一次性命令的 PTY 启动参数。与常驻 Shell 分离,命令执行完即退出。
fn build_command_pty_command(shell: ShellKind, command: &str, cwd: &str) -> CommandBuilder {
    #[cfg(windows)]
    let mut cmd = match shell {
        ShellKind::Cmd => {
            let mut c = CommandBuilder::new("cmd");
            let flat = crate::commands::open::flatten_multiline(Some(command), shell.separator());
            let command = flat.as_deref().unwrap_or(command);
            // portable-pty 按 MSVC 规则把内嵌双引号转义为 \"，cmd 不识别这种转义。
            // 先用环境变量占位,让 cmd 在剥掉 /S /C 的外层引号后再还原,
            // 保留带空格的可执行路径、参数引号及原有 %VAR% 展开语义。
            let mut argument = command.replace('"', "%REPOMEOW_CMD_QUOTE%");
            // MSVC 规则还会翻倍被外层引号包裹的尾部反斜杠,同样延后还原。
            if argument.ends_with('\\') {
                argument.pop();
                argument.push_str("%REPOMEOW_CMD_BACKSLASH%");
            }
            c.args(["/D", "/S", "/C"]);
            c.arg(argument);
            c.env("REPOMEOW_CMD_QUOTE", "\"");
            c.env("REPOMEOW_CMD_BACKSLASH", "\\");
            c
        }
        ShellKind::PowerShell => {
            let ps = crate::commands::open::find_powershell().unwrap_or("powershell");
            let script =
                format!("[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; {command}");
            let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(utf16);
            let mut c = CommandBuilder::new(ps);
            c.args(["-NoLogo", "-NoProfile", "-EncodedCommand", &b64]);
            c
        }
        ShellKind::GitBash => {
            let bash = crate::commands::open::find_git_bash().unwrap_or_else(|| "bash".into());
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(command.as_bytes());
            let mut c = CommandBuilder::new(bash);
            c.arg("-c");
            c.arg(format!("source <(echo {b64} | base64 -d)"));
            c
        }
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let _ = shell;
        let mut c = CommandBuilder::new("sh");
        c.arg("-c");
        c.arg(command);
        c
    };
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd
}

/// 交互式 shell 的 ConPTY 启动参数。伪终端下提示符/回显/行编辑由 shell 自绘
/// (pwsh 走 PSReadLine,cmd 走 conhost 行输入,bash 走 readline),
/// 不再需要管道模式的 `-Command -` 静默读法与前端逐行编辑。
fn build_interactive_pty_command(shell: ShellKind, cwd: &str) -> CommandBuilder {
    #[cfg(windows)]
    let mut cmd = match shell {
        ShellKind::Cmd => {
            let mut c = CommandBuilder::new("cmd");
            c.arg("/K");
            c
        }
        ShellKind::PowerShell => {
            let ps = crate::commands::open::find_powershell().unwrap_or("powershell");
            let mut c = CommandBuilder::new(ps);
            c.args(["-NoLogo", "-NoProfile"]);
            c
        }
        ShellKind::GitBash => {
            let bash = crate::commands::open::find_git_bash().unwrap_or_else(|| "bash".into());
            let mut c = CommandBuilder::new(bash);
            c.arg("-i");
            c
        }
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let _ = shell;
        let mut c = CommandBuilder::new("sh");
        c.arg("-i");
        c
    };
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd
}

fn spawn_err(e: impl std::fmt::Display) -> AppError {
    AppError::coded(ErrorCode::TerminalSpawnFailed, e.to_string())
}

/// 所有会话均通过 PTY 拉起,保证子进程检测到 TTY 并启用交互能力。
fn launch_session(app: &AppHandle, spec: &SessionSpec) -> AppResult<Launched> {
    let shell = spec.shell.unwrap_or_else(|| resolve_shell(app));
    let mut cmd = if spec.interactive {
        build_interactive_pty_command(shell, &spec.cwd)
    } else {
        build_command_pty_command(shell, &spec.command, &spec.cwd)
    };
    // CommandBuilder 在 Windows 构造时已实时读取注册表环境;显式 JAVA_HOME 最后覆盖。
    if let Some(home) = spec
        .java_home
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.env("JAVA_HOME", home.replace('"', ""));
    }
    launch_pty(cmd)
}

/// 独立于 AppHandle 的 PTY 启动路径,便于对真实子进程交互做回归验证。
fn launch_pty(cmd: CommandBuilder) -> AppResult<Launched> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(spawn_err)?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(spawn_err)?));
    // 输出经查询拦截器转发:ESC[6n 即答应答并剥离,防止前端迟到应答被显示
    let reader = PtyQueryReader {
        inner: pair.master.try_clone_reader().map_err(spawn_err)?,
        answers: spawn_answer_writer(writer.clone()),
        seq: Vec::new(),
        ready: VecDeque::new(),
    };
    let child = pair.slave.spawn_command(cmd).map_err(spawn_err)?;
    // 从端句柄在 spawn 后立即释放,避免句柄占用影响子进程退出检测
    drop(pair.slave);
    Ok(Launched {
        child,
        readers: vec![Box::new(reader)],
        writer: Some(writer),
        master: Some(pair.master),
    })
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

/// 光标查询应答写入线程主体:从队列取应答写入 PTY 输入端,写失败(会话已结束)静默忽略。
/// 队列由读取线程持有发送端,读取线程结束(EOF)后发送端 drop、队列关闭,本线程排空后退出
fn answer_writer_loop(rx: mpsc::Receiver<Vec<u8>>, writer: Arc<Mutex<Box<dyn Write + Send>>>) {
    for answer in rx {
        if let Ok(mut guard) = writer.lock() {
            let _ = guard.write_all(&answer);
            let _ = guard.flush();
        }
    }
}

/// 启动应答写入专职线程,返回发送端(交给 PtyQueryReader 持有)。
/// 应答不能在输出读取线程里同步写:输入管道写满时写入会阻塞,读取线程一旦停摆,
/// ConPTY 输出失去排水 → conhost/子进程输出背压 → 子进程更不会读输入,三方互锁;
/// 专职线程被拖住只影响应答时延,不传导回输出排水
fn spawn_answer_writer(writer: Arc<Mutex<Box<dyn Write + Send>>>) -> mpsc::Sender<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || answer_writer_loop(rx, writer));
    tx
}

/// PTY 输出查询拦截:shell(PSReadLine 等)会向终端发光标位置查询 `ESC[6n` 并
/// 短超时等待应答。若交由前端 xterm 应答,一个 IPC 往返的延迟即超过等待窗口,
/// 迟到的应答会被 shell 当作键入文本显示出来(表现为提示符后出现 `[1;1R`)。
/// 因此在后端即刻应答(经专职线程写入输入端,读取线程不做任何输入写),
/// 并把查询本体从转发流剥离(xterm 看不到就不会重复应答)。
/// 同理拦截焦点上报开关 `ESC[?1004h/l`:ConPTY 不识别 xterm 回传的焦点事件
/// (`ESC[I`/`ESC[O`),会当作键入文本显示在提示符后。
struct PtyQueryReader {
    inner: Box<dyn Read + Send>,
    /// 光标查询应答队列的发送端,由专职线程取出写入 PTY 输入端
    answers: mpsc::Sender<Vec<u8>>,
    /// ESC 序列累积缓冲(空 = 普通转发态);查询可能被 read 切开,需跨块拼接
    seq: Vec<u8>,
    /// 已决待转发字节
    ready: VecDeque<u8>,
}

impl PtyQueryReader {
    /// DEC 私有模式开关(ESC[?params h/l)是否包含焦点上报(1004)
    fn is_focus_report_toggle(seq: &[u8]) -> bool {
        let Some(rest) = seq.strip_prefix(b"\x1b[?") else {
            return false;
        };
        let params = rest.strip_suffix(b"h").or_else(|| rest.strip_suffix(b"l"));
        let Some(params) = params else {
            return false;
        };
        std::str::from_utf8(params)
            .map(|s| s.split(';').any(|p| p == "1004"))
            .unwrap_or(false)
    }

    /// seq 已完整(或放弃匹配):查询即答应答、焦点上报开关丢弃,其余原样放行
    fn resolve_seq(&mut self) {
        if self.seq == b"\x1b[6n" {
            // 应答内容仅用于能力探测,shell 不依赖真实光标坐标;入队即返回,
            // 实际写入见 answer_writer_loop
            let _ = self.answers.send(b"\x1b[1;1R".to_vec());
        } else if Self::is_focus_report_toggle(&self.seq) {
            // 丢弃开关本身即可阻止 xterm 发送焦点事件
        } else {
            let seq = std::mem::take(&mut self.seq);
            self.ready.extend(&seq);
            return;
        }
        self.seq.clear();
    }

    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.seq.is_empty() && b != 0x1b {
                self.ready.push_back(b);
                continue;
            }
            self.seq.push(b);
            match self.seq.len() {
                // ESC 单独出现:等下一字节判断是否 CSI
                1 => {}
                // ESC + '[':CSI 头,继续累积参数(注意 '[' 本身在终止字节范围内)
                2 if b == b'[' => {}
                // CSI 终止字节(0x40~0x7e)到达,序列完整
                _ if (0x40..=0x7e).contains(&b) => self.resolve_seq(),
                // ESC + 其他单字符(如 ESC 7):到达终止范围或长度兜底时放行
                // 异常长序列兜底放行,避免缓冲无限增长
                _ if self.seq.len() >= 32 => self.resolve_seq(),
                _ => {}
            }
        }
    }
}

impl Read for PtyQueryReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.ready.is_empty() {
                let mut buf = [0u8; 4096];
                match self.inner.read(&mut buf)? {
                    // 流结束:未决的暂存字节原样交出
                    0 => {
                        if !self.seq.is_empty() {
                            let seq = std::mem::take(&mut self.seq);
                            self.ready.extend(&seq);
                        }
                        if self.ready.is_empty() {
                            return Ok(0);
                        }
                    }
                    n => self.push(&buf[..n]),
                }
                continue;
            }
            let n = self.ready.len().min(out.len());
            let taken: Vec<u8> = self.ready.drain(..n).collect();
            out[..n].copy_from_slice(&taken);
            return Ok(n);
        }
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

fn spawn_waiter(
    app: AppHandle,
    mgr: Arc<TerminalManager>,
    id: u64,
    generation: u64,
    mut child: SessionChild,
) {
    std::thread::spawn(move || {
        let exit_code = child.wait().ok().map(|status| {
            if status.success() {
                0
            } else {
                status.exit_code() as i32
            }
        });
        let info = {
            let mut sessions = mgr.lock();
            let Some(entry) = sessions.get_mut(&id) else {
                return; // 会话已被移除
            };
            if entry.generation != generation {
                return; // 旧进程退出(restart 后),状态已由新代数接管
            }
            entry.writer = None;
            entry.master = None; // 关闭 PTY 主端,释放 ConPTY 句柄
            entry.info.finished_at = Some(now_ts());
            if entry.stop_requested {
                entry.info.status = TerminalSessionStatus::Stopped;
            } else {
                entry.info.exit_code = exit_code;
                entry.info.status = TerminalSessionStatus::Exited;
            }
            entry.info.clone()
        };
        let _ = app.emit(SESSION_CHANGED_EVENT, &info);
    });
}

/// 登记新会话并接管子进程句柄(reader/waiter 线程 + 事件广播)
fn register_launch(
    app: &AppHandle,
    mgr: &Arc<TerminalManager>,
    generation: u64,
    info: TerminalSessionInfo,
    spec: SessionSpec,
    launched: Launched,
) -> TerminalSessionInfo {
    let id = info.id;
    let Launched {
        child,
        readers,
        writer,
        master,
    } = launched;
    let pid = child.process_id().unwrap_or(0);
    {
        let mut sessions = mgr.lock();
        sessions.insert(
            id,
            SessionEntry {
                info: info.clone(),
                generation,
                pid,
                writer,
                master,
                spec,
                stop_requested: false,
                output: VecDeque::new(),
                output_chars: 0,
            },
        );
    }
    for reader in readers {
        spawn_reader(app.clone(), mgr.clone(), id, generation, reader);
    }
    spawn_waiter(app.clone(), mgr.clone(), id, generation, child);
    let _ = app.emit(SESSION_CHANGED_EVENT, &info);
    info
}

/// 在项目内置终端执行命令。
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
    run_command_session_impl(
        &app,
        manager.inner(),
        project_id,
        &project_name,
        &path,
        &command,
        cwd.as_deref(),
        java_home.as_deref(),
        kind.as_deref(),
        label.as_deref(),
    )
}

/// 内嵌命令会话的创建入口:前端 IPC 与项目问答工具共用。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_command_session_impl(
    app: &AppHandle,
    manager: &Arc<TerminalManager>,
    project_id: i64,
    project_name: &str,
    path: &str,
    command: &str,
    cwd: Option<&str>,
    java_home: Option<&str>,
    kind: Option<&str>,
    label: Option<&str>,
) -> AppResult<TerminalSessionInfo> {
    let work_dir = cwd.unwrap_or(path);
    if !std::path::Path::new(&work_dir).is_dir() {
        return Err(AppError::coded(ErrorCode::ScriptDirNotFound, work_dir));
    }
    let mgr = manager.clone();
    let id = mgr.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let spec = SessionSpec {
        command: command.to_string(),
        cwd: work_dir.to_string(),
        java_home: java_home.map(str::to_string),
        interactive: false,
        shell: None,
    };
    let launched = launch_session(app, &spec)?;
    let info = TerminalSessionInfo {
        id,
        project_id,
        project_name: project_name.to_string(),
        label: label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| spec.command.clone()),
        kind: kind.unwrap_or_else(|| "shell").to_string(),
        command: spec.command.clone(),
        interactive: false,
        cwd: spec.cwd.clone(),
        status: TerminalSessionStatus::Running,
        exit_code: None,
        started_at: now_ts(),
        finished_at: None,
    };
    let info = register_launch(app, &mgr, 0, info, spec, launched);
    mgr.prune();
    Ok(info)
}

/// 在当前项目(或选中的 worktree)目录手动打开可输入命令的 shell 会话。
/// shell 为前端显式选择的类型(cmd/powershell/gitbash),缺省按设置项;不可用时回退 cmd。
#[tauri::command]
pub fn create_shell_session(
    app: AppHandle,
    manager: State<'_, Arc<TerminalManager>>,
    project_id: i64,
    project_name: String,
    path: String,
    shell: Option<String>,
) -> AppResult<TerminalSessionInfo> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(AppError::coded(ErrorCode::ScriptDirNotFound, path));
    }
    let mgr = manager.inner().clone();
    let id = mgr.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let shell_kind = match shell {
        Some(choice) => resolve_shell_choice(Some(choice)),
        None => resolve_shell(&app),
    };
    #[cfg(windows)]
    let label = shell_kind.label().to_string();
    #[cfg(not(windows))]
    let label = "Shell".to_string();
    let spec = SessionSpec {
        command: String::new(),
        cwd: path,
        java_home: None,
        interactive: true,
        shell: Some(shell_kind),
    };
    let launched = launch_session(&app, &spec)?;
    let info = TerminalSessionInfo {
        id,
        project_id,
        project_name,
        label: label.clone(),
        kind: "shell".into(),
        command: label,
        interactive: true,
        cwd: spec.cwd.clone(),
        status: TerminalSessionStatus::Running,
        exit_code: None,
        started_at: now_ts(),
        finished_at: None,
    };
    let info = register_launch(&app, &mgr, 0, info, spec, launched);
    mgr.prune();
    Ok(info)
}

/// 全部会话:运行中优先,其余按启动时间倒序
#[tauri::command]
pub fn list_command_sessions(manager: State<'_, Arc<TerminalManager>>) -> Vec<TerminalSessionInfo> {
    list_command_sessions_impl(manager.inner())
}

/// 全部会话:运行中优先,其余按启动时间倒序。
pub(crate) fn list_command_sessions_impl(
    manager: &Arc<TerminalManager>,
) -> Vec<TerminalSessionInfo> {
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
    get_command_session_output_impl(manager.inner(), id)
}

/// 回放会话输出缓冲(前端打开详情页时一次性拉取,此后走事件增量)。
pub(crate) fn get_command_session_output_impl(
    manager: &Arc<TerminalManager>,
    id: u64,
) -> AppResult<String> {
    let sessions = manager.lock();
    let entry = sessions.get(&id).ok_or_else(|| session_not_found(id))?;
    Ok(entry.output.iter().map(String::as_str).collect())
}

/// 停止会话(整棵树);进程退出后 waiter 把状态归为 stopped
#[tauri::command]
pub fn stop_command_session(manager: State<'_, Arc<TerminalManager>>, id: u64) -> AppResult<()> {
    stop_command_session_impl(manager.inner(), id)
}

/// 停止会话(整棵树);进程退出后 waiter 把状态归为 stopped。
pub(crate) fn stop_command_session_impl(manager: &Arc<TerminalManager>, id: u64) -> AppResult<()> {
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
    restart_command_session_impl(&app, manager.inner(), id)
}

/// 重启会话:运行中先整棵树停止,再以原参数重新拉起;会话 id 不变,
/// 输出缓冲清空,代数递增使旧线程迟到事件失效。前端 IPC 与项目问答工具共用。
pub(crate) fn restart_command_session_impl(
    app: &AppHandle,
    manager: &Arc<TerminalManager>,
    id: u64,
) -> AppResult<TerminalSessionInfo> {
    let mgr = manager.clone();
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
            shell: entry.spec.shell,
        }
    };
    let launched = match launch_session(app, &spec) {
        Ok(l) => l,
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
    let Launched {
        child,
        readers,
        writer,
        master,
    } = launched;
    let (info, generation) = {
        let mut sessions = mgr.lock();
        let entry = sessions.get_mut(&id).ok_or_else(|| session_not_found(id))?;
        entry.generation += 1;
        entry.pid = child.process_id().unwrap_or(0);
        entry.writer = writer;
        entry.master = master;
        entry.stop_requested = false;
        entry.output.clear();
        entry.output_chars = 0;
        entry.info.status = TerminalSessionStatus::Running;
        entry.info.exit_code = None;
        entry.info.started_at = now_ts();
        entry.info.finished_at = None;
        (entry.info.clone(), entry.generation)
    };
    for reader in readers {
        spawn_reader(app.clone(), mgr.clone(), id, generation, reader);
    }
    spawn_waiter(app.clone(), mgr.clone(), id, generation, child);
    let _ = app.emit(SESSION_CHANGED_EVENT, &info);
    Ok(info)
}

/// PTY 输入统一换行:Enter、LF/CRLF 多行粘贴均映射为单个 CR。
/// 焦点上报不是键盘输入,不应写进 ConPTY 被显示成字面字符。
fn normalize_pty_input(data: &str) -> Option<String> {
    if data == "\x1b[I" || data == "\x1b[O" {
        return None;
    }
    Some(data.replace("\r\n", "\n").replace('\n', "\r"))
}

/// 向会话 PTY 输入端写入(xterm 键盘输入);会话已结束时静默忽略。
#[tauri::command]
pub fn write_command_session(
    manager: State<'_, Arc<TerminalManager>>,
    id: u64,
    data: String,
) -> AppResult<()> {
    write_command_session_impl(manager.inner(), id, &data)
}

/// 前端键盘输入与 CLI 输入共用的 PTY 写入入口。
pub(crate) fn write_command_session_impl(
    manager: &Arc<TerminalManager>,
    id: u64,
    data: &str,
) -> AppResult<()> {
    let writer = {
        let sessions = manager.lock();
        let entry = sessions.get(&id).ok_or_else(|| session_not_found(id))?;
        entry.writer.clone()
    };
    if let (Some(writer), Some(input)) = (writer, normalize_pty_input(data)) {
        let mut guard = writer.lock().unwrap();
        // 子进程已退出但 waiter 尚未清理句柄时写入会 EPIPE,按静默处理。
        let _ = guard.write_all(input.as_bytes());
        let _ = guard.flush();
    }
    Ok(())
}

/// 同步会话的 PTY 尺寸(前端 xterm fit 后调用)。
#[tauri::command]
pub fn resize_command_session(
    manager: State<'_, Arc<TerminalManager>>,
    id: u64,
    rows: u16,
    cols: u16,
) -> AppResult<()> {
    let sessions = manager.lock();
    let entry = sessions.get(&id).ok_or_else(|| session_not_found(id))?;
    if let Some(master) = &entry.master {
        let _ = master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
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
    fn pty_input_normalizes_enter_and_paste_without_duplicate_lines() {
        assert_eq!(normalize_pty_input("o\r").as_deref(), Some("o\r"));
        assert_eq!(
            normalize_pty_input("a\nb\r\nc\r").as_deref(),
            Some("a\rb\rc\r")
        );
        assert_eq!(normalize_pty_input("\x03").as_deref(), Some("\x03"));
        assert_eq!(normalize_pty_input("\x1b[A").as_deref(), Some("\x1b[A"));
        assert_eq!(normalize_pty_input("\x1b[I"), None);
        assert_eq!(normalize_pty_input("\x1b[O"), None);
    }

    /// 使用真实 PTY 读写,同时验证命令退出后不会留下常驻 Shell。
    fn exercise_pty_command(cmd: CommandBuilder, input: Option<(&str, &str)>) -> (String, i32) {
        let Launched {
            mut child,
            readers,
            writer,
            mut master,
        } = launch_pty(cmd).unwrap();
        let (tx, rx) = mpsc::channel();
        for mut reader in readers {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut output = Vec::new();
        let mut sent = false;
        let mut exit_code = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(chunk) => output.extend(chunk),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) if exit_code.is_some() => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some((prompt, data)) = input {
                if !sent && String::from_utf8_lossy(&output).contains(prompt) {
                    let mut guard = writer.as_ref().unwrap().lock().unwrap();
                    guard
                        .write_all(normalize_pty_input(data).unwrap().as_bytes())
                        .unwrap();
                    guard.flush().unwrap();
                    sent = true;
                }
            }
            if exit_code.is_none() {
                if let Some(status) = child.try_wait().unwrap() {
                    exit_code = Some(status.exit_code() as i32);
                    // ConPTY 的最后一屏在关闭主端时排空,reader 继续收集到 EOF。
                    master.take();
                }
            }
        }
        let text = String::from_utf8_lossy(&output).into_owned();
        if exit_code.is_none() {
            // reader 也可能先于 try_wait 观察到退出;有界等待,失败时杀掉测试进程。
            while std::time::Instant::now() < deadline {
                if let Some(status) = child.try_wait().unwrap() {
                    exit_code = Some(status.exit_code() as i32);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if exit_code.is_none() {
                kill_tree(child.process_id().unwrap_or(0));
                let _ = child.wait();
            }
        }
        drop(master);
        assert!(input.is_none() || sent, "未收到输入提示: {text:?}");
        (
            text.clone(),
            exit_code.unwrap_or_else(|| panic!("命令未自动退出: {text:?}")),
        )
    }

    #[test]
    fn command_pty_accepts_input_and_preserves_exit_code() {
        #[cfg(windows)]
        let command = "set /p REPOMEOW_INPUT=READY_FOR_INPUT: & set REPOMEOW_INPUT & exit /b 7";
        #[cfg(not(windows))]
        let command = "printf READY_FOR_INPUT:; read REPOMEOW_INPUT; printf 'REPOMEOW_INPUT=%s\\n' \"$REPOMEOW_INPUT\"; exit 7";
        let cmd = build_command_pty_command(ShellKind::Cmd, command, ".");
        let (output, exit_code) =
            exercise_pty_command(cmd, Some(("READY_FOR_INPUT:", "hello-pty\r")));
        assert!(
            output.contains("REPOMEOW_INPUT=hello-pty"),
            "输入未被命令处理: {output:?}"
        );
        assert_eq!(exit_code, 7);
    }

    #[cfg(windows)]
    #[test]
    fn command_pty_cmd_preserves_quotes_variables_and_trailing_backslash() {
        let command = "echo \"quoted %REPOMEOW_TEST_VALUE%\"\necho C:\\test\\";
        let mut cmd = build_command_pty_command(ShellKind::Cmd, command, ".");
        cmd.env("REPOMEOW_TEST_VALUE", "expanded value");
        let (output, exit_code) = exercise_pty_command(cmd, None);
        assert!(
            output.contains("\"quoted expanded value\""),
            "引号或变量展开错误: {output:?}"
        );
        assert!(
            output.contains("C:\\test\\\r\n"),
            "尾部反斜杠被改写: {output:?}"
        );
        assert_eq!(exit_code, 0);
    }

    #[cfg(windows)]
    #[test]
    fn command_pty_cmd_supports_quoted_executable_path() {
        let command = format!("\"{}\" /D /C exit 9", std::env::var("COMSPEC").unwrap());
        let (output, exit_code) = exercise_pty_command(
            build_command_pty_command(ShellKind::Cmd, &command, "."),
            None,
        );
        assert_eq!(exit_code, 9, "带引号的可执行路径未正确执行: {output:?}");
    }

    #[cfg(windows)]
    #[test]
    fn command_pty_powershell_sees_console_and_accepts_input() {
        let command = "Write-Output ('TTY:' + (-not [Console]::IsInputRedirected) + ':' + (-not [Console]::IsOutputRedirected)); $value = Read-Host 'READY_FOR_INPUT:'; Write-Output ('INPUT_RECEIVED:' + $value); exit 6";
        let (output, exit_code) = exercise_pty_command(
            build_command_pty_command(ShellKind::PowerShell, command, "."),
            Some(("READY_FOR_INPUT:", "hello-pty\r")),
        );
        assert!(
            output.contains("TTY:True:True"),
            "子进程未检测到 TTY: {output:?}"
        );
        assert!(
            output.contains("INPUT_RECEIVED:hello-pty"),
            "输入未被命令处理: {output:?}"
        );
        assert_eq!(exit_code, 6);
    }

    #[cfg(windows)]
    #[test]
    fn command_pty_gitbash_sees_tty_and_accepts_input() {
        // Git Bash 是可选 Shell;没有安装时由生产路径回退到 cmd。
        if crate::commands::open::find_git_bash().is_none() {
            return;
        }
        let command = "test -t 0 && test -t 1 && printf 'TTY:yes\\n'; printf READY_FOR_INPUT:; IFS= read -r value; printf 'INPUT_RECEIVED:%s\\n' \"$value\"; exit 8";
        let (output, exit_code) = exercise_pty_command(
            build_command_pty_command(ShellKind::GitBash, command, "."),
            Some(("READY_FOR_INPUT:", "hello-pty\r")),
        );
        assert!(output.contains("TTY:yes"), "子进程未检测到 TTY: {output:?}");
        assert!(
            output.contains("INPUT_RECEIVED:hello-pty"),
            "输入未被命令处理: {output:?}"
        );
        assert_eq!(exit_code, 8);
    }

    /// 需要本机已安装 Node/pnpm 和前端依赖;不启动浏览器,只验证真实 Vite CLI 的 q 指令。
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn command_pty_vite_shortcut_smoke() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let mut cmd = build_command_pty_command(
            ShellKind::Cmd,
            "pnpm exec vite --host 127.0.0.1 --port 0 --strictPort --clearScreen false",
            root.to_str().unwrap(),
        );
        cmd.env_remove("CI");
        let (output, exit_code) = exercise_pty_command(cmd, Some(("show help", "q\r")));
        assert!(output.contains("Local:"), "Vite 启动失败: {output:?}");
        assert_eq!(exit_code, 0, "Vite 快捷键退出失败: {output:?}");
    }

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
        // "中文" 的 GBK 编码:D6 D0 CE C4(C4 C4 是"哪",曾因测试数据写错而恒失败)
        assert_eq!(d.push(&[0xD6, 0xD0, 0xCE, 0xC4]), "中文");
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
            writer: None,
            master: None,
            spec: SessionSpec {
                command: "c".into(),
                cwd: ".".into(),
                java_home: None,
                interactive: false,
                shell: None,
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

    /// 渲染行为诊断(手动):`cargo test -- --ignored pty_resize_diag --nocapture`。
    /// 模拟前端时序(spawn 后立刻 resize 到面板行数 → 执行真实命令),
    /// 转储原始输出流,用于核对 ConPTY 的重绘/清屏序列。
    #[test]
    #[ignore]
    fn pty_resize_diag() {
        // 临时目录放一个无依赖的 package.json,模拟 pnpm i 快速路径
        let tmp = std::env::temp_dir().join("repomeow-pty-diag");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(
            tmp.join("package.json"),
            "{\"name\":\"diag\",\"private\":true}",
        )
        .unwrap();
        let tmp_str = tmp.to_string_lossy().to_string();

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
        let mut reader = PtyQueryReader {
            inner: pair.master.try_clone_reader().unwrap(),
            answers: spawn_answer_writer(writer.clone()),
            seq: Vec::new(),
            ready: VecDeque::new(),
        };
        let mut child = pair
            .slave
            .spawn_command(build_interactive_pty_command(
                ShellKind::PowerShell,
                &tmp_str,
            ))
            .unwrap();
        // spawn 后立刻 resize,与前端挂载时序一致
        let _ = pair.master.resize(PtySize {
            rows: 16,
            cols: 110,
            pixel_width: 0,
            pixel_height: 0,
        });
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut drain = |secs: u64, label: &str, output: &mut Vec<u8>| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
            while std::time::Instant::now() < deadline {
                if let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    output.extend_from_slice(&chunk);
                }
            }
            let text = String::from_utf8_lossy(output);
            eprintln!("=== {label} ===\n{}", text.escape_default());
        };
        let run = |cmd: &str, writer: &Arc<Mutex<Box<dyn Write + Send>>>| {
            if let Ok(mut guard) = writer.lock() {
                let _ = guard.write_all(cmd.as_bytes());
            }
        };
        let mut output = Vec::new();
        drain(3, "启动(resize 后)", &mut output);
        run("pnpm -v\r", &writer);
        drain(3, "pnpm -v", &mut output);
        run("1..40 | ForEach-Object { \"line-$_\" }\r", &writer);
        drain(4, "灌满 40 行(视口 16 行)", &mut output);
        run("echo step-x\r", &writer);
        drain(3, "满屏后执行命令", &mut output);
        let _ = child.kill();
    }

    /// 手动冒烟(需本机装有 pwsh):`cargo test -- --ignored pty_pwsh_smoke`。
    /// 经 ConPTY 真实拉起 PowerShell,验证 PtyQueryReader 自动应答光标查询后
    /// 提示符正常出现、命令可执行,且查询/应答不泄露到转发流。
    #[test]
    #[ignore]
    fn pty_pwsh_smoke() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
        let mut reader = PtyQueryReader {
            inner: pair.master.try_clone_reader().unwrap(),
            answers: spawn_answer_writer(writer.clone()),
            seq: Vec::new(),
            ready: VecDeque::new(),
        };
        let mut child = pair
            .slave
            .spawn_command(build_interactive_pty_command(ShellKind::PowerShell, "."))
            .unwrap();
        // 读线程持续汇集输出,主线程带超时轮询检查
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut output: Vec<u8> = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut prompted = false;
        let mut done = false;
        while std::time::Instant::now() < deadline {
            if let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                output.extend_from_slice(&chunk);
            }
            let text = String::from_utf8_lossy(&output);
            if !prompted && text.contains("PS ") {
                prompted = true;
                if let Ok(mut guard) = writer.lock() {
                    guard.write_all(b"echo pty-ok\r").unwrap();
                }
            }
            // 回显里也会出现 pty-ok,以第二个提示符(命令执行完毕)为准
            if prompted && text.matches("PS ").count() >= 2 && text.contains("pty-ok") {
                done = true;
                break;
            }
        }
        let _ = child.kill();
        let text = String::from_utf8_lossy(&output);
        assert!(prompted, "未收到 PowerShell 提示符: {text:?}");
        assert!(done, "未收到 echo 执行结果: {text:?}");
        // 查询/应答/焦点上报开关都不应出现在转发流(否则会被 shell 当作键入文本显示)
        assert!(!text.contains("[6n"), "光标查询未被剥离: {text:?}");
        assert!(!text.contains("[1;1R"), "应答泄露到转发流: {text:?}");
        assert!(!text.contains("1004"), "焦点上报开关未被剥离: {text:?}");
    }

    /// 拦截器核心行为:光标查询即答应答(入队)、焦点上报开关丢弃,其余序列原样透传
    /// 记录型 mock writer:应答线程写入内容进共享缓冲供断言
    struct RecordingWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn recording_writer() -> (Arc<Mutex<Box<dyn Write + Send>>>, Arc<Mutex<Vec<u8>>>) {
        let recorded = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(RecordingWriter(recorded.clone()))));
        (writer, recorded)
    }

    /// 拦截器核心行为:光标查询即答应答、焦点上报开关丢弃,其余序列原样透传
    #[test]
    fn pty_query_filter_answers_and_strips() {
        let (answers, answer_rx) = mpsc::channel();
        let mut filter = PtyQueryReader {
            inner: Box::new(std::io::Cursor::new(
                b"hi\x1b[6nthere\x1b[?1004h\x1b[?9001h!".to_vec(),
            )),
            answers,
            seq: Vec::new(),
            ready: VecDeque::new(),
        };
        let mut out = Vec::new();
        filter.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hithere\x1b[?9001h!");
        assert_eq!(answer_rx.try_recv(), Ok(b"\x1b[1;1R".to_vec()));
        // 查询之后只有焦点开关被丢弃,不应再产生任何应答
        assert_eq!(
            answer_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "焦点上报开关不应触发应答"
        );
    }

    /// 查询序列被 read 切开时仍要能拼出并拦截(逐字节喂入)
    #[test]
    fn pty_query_filter_handles_split_sequences() {
        struct OneByteReader {
            data: Vec<u8>,
            pos: usize,
        }
        impl Read for OneByteReader {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.pos < self.data.len() {
                    out[0] = self.data[self.pos];
                    self.pos += 1;
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
        }
        let (answers, answer_rx) = mpsc::channel();
        let mut filter = PtyQueryReader {
            inner: Box::new(OneByteReader {
                data: b"a\x1b[6nb\x1b[?1004hc".to_vec(),
                pos: 0,
            }),
            answers,
            seq: Vec::new(),
            ready: VecDeque::new(),
        };
        let mut out = Vec::new();
        filter.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"abc");
        assert_eq!(answer_rx.try_recv(), Ok(b"\x1b[1;1R".to_vec()));
    }

    /// 应答写入线程:排空队列逐条写入输入端,队列关闭后退出
    #[test]
    fn answer_writer_loop_drains_then_exits() {
        let (writer, recorded) = recording_writer();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let handle = std::thread::spawn(move || answer_writer_loop(rx, writer));
        tx.send(b"\x1b[1;1R".to_vec()).unwrap();
        drop(tx);
        handle.join().unwrap();
        assert_eq!(*recorded.lock().unwrap(), b"\x1b[1;1R");
    }

    /// 焦点上报开关识别:单参数/组合参数拦截,其余模式放行
    #[test]
    fn filter_focus_report_toggle() {
        assert!(PtyQueryReader::is_focus_report_toggle(b"\x1b[?1004h"));
        assert!(PtyQueryReader::is_focus_report_toggle(b"\x1b[?1004l"));
        assert!(PtyQueryReader::is_focus_report_toggle(b"\x1b[?2004;1004h"));
        assert!(!PtyQueryReader::is_focus_report_toggle(b"\x1b[?9001h"));
        assert!(!PtyQueryReader::is_focus_report_toggle(b"\x1b[?25h"));
        assert!(!PtyQueryReader::is_focus_report_toggle(b"\x1b[6n"));
    }
}
