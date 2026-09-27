//! 项目内嵌终端:在项目目录内直接执行命令(npm scripts / docker compose / 自定义命令),
//! 输出经 `terminal://output` 事件流式推送前端(xterm 渲染),
//! 支持实时输出查看、stdin 写入、停止(Windows 整棵树 taskkill)与重启。
//! 与「系统终端新窗口」模式(run_in_terminal)并存,由设置项 embeddedTerminal 分流。
//!
//! 两种会话形态:
//! - 命令会话:管道捕获 stdout/stderr(避免 ConPTY 屏幕重绘序列污染输出回放缓冲);
//! - 交互式 Shell:ConPTY 伪终端,提示符/回显/行编辑由 shell 自绘(pwsh 走 PSReadLine)。
//!
//! 会话仅存内存:应用退出即清空,不做持久化。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
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
    /// 命令会话的 stdin 或交互式会话的 PTY 输入端
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    /// 交互式会话的 PTY 主端(resize 用);进程退出后关闭以释放 ConPTY 句柄
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

/// 会话子进程句柄:命令会话是普通管道子进程,交互式是 ConPTY 子进程;
/// 统一 wait/pid 供 waiter 线程与注册表使用
enum SessionChild {
    Piped(std::process::Child),
    Pty(Box<dyn portable_pty::Child + Send + Sync>),
}

impl SessionChild {
    fn pid(&self) -> u32 {
        match self {
            SessionChild::Piped(c) => c.id(),
            SessionChild::Pty(c) => c.process_id().unwrap_or(0),
        }
    }

    /// 等待退出并取退出码(信号终止等无码场景为 None)
    fn wait(&mut self) -> Option<i32> {
        match self {
            SessionChild::Piped(c) => c.wait().ok().and_then(|s| s.code()),
            // portable-pty 的退出码为 u32,截回 i32 与管道会话同型
            SessionChild::Pty(c) => c
                .wait()
                .ok()
                .map(|s| if s.success() { 0 } else { s.exit_code() as i32 }),
        }
    }
}

/// 一次拉起的全部句柄:子进程 + 输出流(PTY 单流,管道 stdout/stderr 双流)+ 输入端 + PTY 主端
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

/// 按会话形态拉起子进程:交互式走 ConPTY(初值 120x30,前端 fit 后经 resize 同步),
/// 命令会话走管道
fn launch_session(app: &AppHandle, spec: &SessionSpec) -> AppResult<Launched> {
    if !spec.interactive {
        let mut cmd = build_shell_command(app, &spec.command);
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
        let mut child = cmd.spawn().map_err(spawn_err)?;
        let writer = child
            .stdin
            .take()
            .map(|s| Arc::new(Mutex::new(Box::new(s) as Box<dyn Write + Send>)));
        let readers = [
            child.stdout.take().map(|r| Box::new(r) as Box<dyn Read + Send>),
            child.stderr.take().map(|r| Box::new(r) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        return Ok(Launched {
            child: SessionChild::Piped(child),
            readers,
            writer,
            master: None,
        });
    }

    let shell = spec.shell.unwrap_or_else(|| resolve_shell(app));
    let mut cmd = build_interactive_pty_command(shell, &spec.cwd);
    if let Some(home) = spec.java_home.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cmd.env("JAVA_HOME", home.replace('"', ""));
    }
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
        child: SessionChild::Pty(child),
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
        let exit_code = child.wait();
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
    let pid = child.pid();
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
        shell: None,
    };
    let launched = launch_session(&app, &spec)?;
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
    let info = register_launch(&app, &mgr, 0, info, spec, launched);
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
            shell: entry.spec.shell,
        }
    };
    let launched = match launch_session(&app, &spec) {
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
        entry.pid = child.pid();
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

/// 向会话输入端写入(xterm 键盘输入);会话已结束时静默忽略。
/// 交互式 PTY 会话按终端输入语义把换行统一成 \r(Enter/粘贴多行);
/// 命令会话原样透传 stdin。
#[tauri::command]
pub fn write_command_session(
    manager: State<'_, Arc<TerminalManager>>,
    id: u64,
    data: String,
) -> AppResult<()> {
    let (writer, interactive) = {
        let sessions = manager.lock();
        let entry = sessions.get(&id).ok_or_else(|| session_not_found(id))?;
        (entry.writer.clone(), entry.spec.interactive)
    };
    if let Some(writer) = writer {
        let mut guard = writer.lock().unwrap();
        // 子进程已退出但 waiter 尚未清理句柄时写入会 EPIPE,按静默处理
        let input = if interactive {
            // ConPTY 不识别焦点上报事件(ESC[I/ESC[O),会当作字面键入显示;
            // 输出端已拦截 1004 开关,此处兜底丢弃。xterm 每次焦点变化都单独
            // 发送完整序列,不会与键盘输入粘连,精确匹配不会误伤 ESC 按键
            if data == "\x1b[I" || data == "\x1b[O" {
                return Ok(());
            }
            data.replace('\n', "\r")
        } else {
            data
        };
        let _ = guard.write_all(input.as_bytes());
        let _ = guard.flush();
    }
    Ok(())
}

/// 同步交互式会话的 PTY 尺寸(前端 xterm fit 后调用);命令会话是管道无尺寸,静默忽略
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
        std::fs::write(tmp.join("package.json"), "{\"name\":\"diag\",\"private\":true}").unwrap();
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
            .spawn_command(build_interactive_pty_command(ShellKind::PowerShell, &tmp_str))
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
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(
            RecordingWriter(recorded.clone()),
        )));
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
