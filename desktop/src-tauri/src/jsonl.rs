//! 一个 stdio JSONL 子进程 + 请求/应答配对（通道 A 与通道 B 共用）。
//!
//! 两条硬要求来自《GUI 工程规格书 v0.2》：
//!
//! * **stdout EOF 即刻判定死亡**（§4.2 / S58）——读线程一看到 EOF 就把 `alive`
//!   置 false 并回调通知界面，不靠轮询、不靠超时。**在途请求也当场作废**
//!   （P1）：死进程要回"死了 + stderr 尾巴"，而不是让调用方等满 60/120/180 s；
//! * **stderr 只作诊断**（§4.9）——落日志文件，界面只拿尾部 N 行。
//!
//! 另外两条工程约束：
//!
//! * **子进程不开控制台窗口**（P4，M2）：`CREATE_NO_WINDOW`；
//! * **事件有界 + 丢弃告警**（P8，§4.9 规则 4）：读取循环只做入队，
//!   队列满了丢最旧的一条并合成一条 `events_dropped`，反压不传回子进程的 stdout；
//! * **单帧有上限**（P9）：超长的一行就地丢弃，不让它把内存吃光。

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

/// 一条 stdout 帧的落点：`None` 表示"这条帧没人认领"（当事件丢弃并记日志）。
pub type FrameSink = Arc<dyn Fn(Value) + Send + Sync>;
/// 子进程死亡（EOF / 退出）时的回调；参数是退出码。
pub type ExitSink = Arc<dyn Fn(Option<i32>, String) + Send + Sync>;

const STDERR_TAIL: usize = 200;

/// 单帧上限（P9）：合法帧是 8 KiB 的事件与几十 KB 的 RPC 应答。
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// 事件队列的容量（P8）。一次 run 约 60 条事件，256 足够；溢出即丢弃并告警。
pub const EVENT_QUEUE_CAPACITY: usize = 256;

/// 子进程死亡的错误码：外壳侧的 `SIDECAR_EXITED`（§4.8 没有它的近亲）。
const EXITED_CODE: &str = "SIDECAR_EXITED";

/// `spawn` 的入参：一个进程的"是什么 + 怎么起 + 帧去哪"。
pub struct SpawnOptions<'a> {
    /// 日志与错误里用的名字（`engine` / `agent`）。
    pub label: &'a str,
    pub program: &'a str,
    pub args: &'a [String],
    pub envs: &'a [(String, String)],
    pub cwd: Option<&'a std::path::Path>,
    /// stderr 的落点（§4.9：诊断只进文件与尾部缓冲）。
    pub log_path: &'a std::path::Path,
    /// 不属于任何请求/应答的帧（Agent 的事件）。
    pub sink: Option<FrameSink>,
    /// stdout EOF / 进程退出时的通知（§4.2：即刻判定死亡）。
    pub on_exit: Option<ExitSink>,
}

pub struct JsonlChild {
    label: String,
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<String, Sender<Value>>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    alive: Arc<AtomicBool>,
}

impl JsonlChild {
    pub fn spawn(options: SpawnOptions<'_>) -> Result<Arc<Self>, String> {
        let SpawnOptions {
            label,
            program,
            args,
            envs,
            cwd,
            log_path,
            sink,
            on_exit,
        } = options;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        for (key, value) in envs {
            command.env(key, value);
        }
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start {label} ({program}): {error}"))?;
        let stdin = child.stdin.take().ok_or("child has no stdin")?;
        let stdout = child.stdout.take().ok_or("child has no stdout")?;
        let stderr = child.stderr.take().ok_or("child has no stderr")?;

        let process = Arc::new(Self {
            label: label.to_owned(),
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            pending: Mutex::new(HashMap::new()),
            stderr_tail: Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL))),
            alive: Arc::new(AtomicBool::new(true)),
        });

        // stdout: one JSON object per line.
        {
            let me = Arc::clone(&process);
            std::thread::Builder::new()
                .name(format!("{label}-stdout"))
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    loop {
                        let next = read_bounded_line(&mut reader, MAX_FRAME_BYTES);
                        let Ok(Some((line, oversized))) = next else {
                            break;
                        };
                        if oversized {
                            // P9：一行超过 8 MiB 就地丢弃，只留一条诊断。
                            me.note_stderr("[oversized frame dropped: > 8 MiB on one line]");
                            continue;
                        }
                        me.route(line.trim(), sink.as_ref());
                    }
                    me.mark_dead("stdout closed");
                    // P1：在途请求当场作废——回"死了 + stderr 尾巴"，不等超时。
                    me.fail_pending("stdout closed");
                    if let Some(callback) = on_exit.as_ref() {
                        let label = me.label.clone();
                        callback(me.exit_code(), label);
                    }
                })
                .map_err(|error| format!("cannot start the {label} reader: {error}"))?;
        }

        // stderr: diagnostics only, mirrored into the log file.
        {
            let me = Arc::clone(&process);
            let log = log_path.to_path_buf();
            std::thread::Builder::new()
                .name(format!("{label}-stderr"))
                .spawn(move || {
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log)
                        .ok();
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        me.note_stderr(&line);
                        if let Some(handle) = file.as_mut() {
                            let _ = writeln!(handle, "{line}");
                        }
                    }
                })
                .map_err(|error| format!("cannot start the {label} stderr reader: {error}"))?;
        }

        Ok(process)
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.lock().ok().and_then(|child| child.id().into())
    }

    pub fn stderr_tail(&self, lines: usize) -> Vec<String> {
        self.stderr_tail
            .lock()
            .map(|tail| tail.iter().rev().take(lines).rev().cloned().collect())
            .unwrap_or_default()
    }

    /// 一条 stdout 帧：先按 `id` 配对在途请求，其余交给事件落点。
    fn route(&self, text: &str, sink: Option<&FrameSink>) {
        if text.is_empty() {
            return;
        }
        let Ok(frame) = serde_json::from_str::<Value>(text) else {
            self.note_stderr(&format!("[non-JSON stdout] {text}"));
            return;
        };
        if let Some(id) = frame.get("id").and_then(Value::as_str) {
            let waiter = self.pending.lock().ok().and_then(|mut map| map.remove(id));
            if let Some(sender) = waiter {
                let _ = sender.send(frame);
                return;
            }
        }
        if let Some(sink) = sink {
            sink(frame);
        }
    }

    /// P1：进程死了就把 `pending` 清空，让每个等待者立刻收到可读错误。
    fn fail_pending(&self, reason: &str) {
        let tail = self.stderr_tail(3).join(" | ");
        let drained: Vec<(String, Sender<Value>)> = match self.pending.lock() {
            Ok(mut map) => map.drain().collect(),
            Err(_) => return,
        };
        for (id, sender) in drained {
            let frame = json!({
                "id": id,
                "ok": false,
                "error": {
                    "code": EXITED_CODE,
                    "message": format!(
                        "{} died before answering ({reason}); stderr: {tail}",
                        self.label
                    ),
                    "retryable": true,
                    "where": self.label,
                },
            });
            let _ = sender.send(frame);
        }
    }

    fn note_stderr(&self, line: &str) {
        if let Ok(mut tail) = self.stderr_tail.lock() {
            if tail.len() >= STDERR_TAIL {
                tail.pop_front();
            }
            tail.push_back(line.to_owned());
        }
    }

    fn mark_dead(&self, reason: &str) {
        if self.alive.swap(false, Ordering::SeqCst) {
            self.note_stderr(&format!("[{} exited: {reason}]", self.label));
        }
    }

    fn exit_code(&self) -> Option<i32> {
        self.child
            .lock()
            .ok()
            .and_then(|mut child| child.try_wait().ok().flatten())
            .and_then(|status| status.code())
    }

    /// 写一帧（不加换行处理：`ensure_ascii=false` + 单行 JSON）。
    pub fn write(&self, frame: &Value) -> Result<(), String> {
        let line = serde_json::to_string(frame).map_err(|error| error.to_string())?;
        let mut guard = self.stdin.lock().map_err(|_| "stdin lock poisoned")?;
        let stdin = guard.as_mut().ok_or_else(|| format!("{} stdin is closed", self.label))?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("cannot write to {}: {error}", self.label))
    }

    /// 发一帧并等它对应的应答；超时/进程死亡都回可读错误。
    pub fn request(&self, frame: Value, id: &str, timeout: Duration) -> Result<Value, String> {
        if !self.alive() {
            return Err(format!(
                "{} is not running: {}",
                self.label,
                self.stderr_tail(3).join(" | ")
            ));
        }
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| "pending lock poisoned")?
            .insert(id.to_owned(), sender);
        if let Err(error) = self.write(&frame) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(id);
            }
            return Err(error);
        }
        match receiver.recv_timeout(timeout) {
            Ok(response) => Ok(response),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(id);
                }
                Err(format!("{} did not answer within {timeout:?}", self.label))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!(
                "{} died before answering ({} )",
                self.label,
                self.stderr_tail(3).join(" | ")
            )),
        }
    }

    pub fn close(&self) {
        if let Ok(mut guard) = self.stdin.lock() {
            // Dropping stdin gives the child its EOF: the polite shutdown.
            *guard = None;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let done = self
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok().flatten())
                .is_some();
            if done || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Ok(mut child) = self.child.lock() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.mark_dead("closed by the shell");
        self.fail_pending("closed by the shell");
    }
}

/// `CREATE_NO_WINDOW`（P4）：sidecar 不该为桌面应用弹出控制台黑框（M2）。
#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`（winbase.h）。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

/// `hide_console` 的公开入口，给只跑一次性 CLI 的调用点（`engine_cli`）复用。
pub fn hide_console_window(command: &mut Command) {
    hide_console(command);
}

/// 读一行；超过 `limit` 字节的一行会被**丢弃**（返回 `oversized = true`），
/// 剩下的字节边读边扔，不驻留内存（P9）。EOF 返回 `Ok(None)`。
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Option<(String, bool)>> {
    let mut buffer: Vec<u8> = Vec::new();
    let read = reader
        .by_ref()
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut buffer)?;
    if read == 0 {
        return Ok(None);
    }
    let oversized = buffer.len() > limit;
    if oversized && !buffer.ends_with(b"\n") {
        let mut scratch: Vec<u8> = Vec::new();
        loop {
            scratch.clear();
            let chunk = reader
                .by_ref()
                .take(64 * 1024)
                .read_until(b'\n', &mut scratch)?;
            if chunk == 0 || scratch.ends_with(b"\n") {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&buffer)
        .trim_end_matches(['\n', '\r'])
        .to_owned();
    Ok(Some((text, oversized)))
}

/// 有界事件队列（P8，《GUI 工程规格书 v0.2》§4.9 规则 4）。
///
/// 读取循环只做 `push`（不阻塞、不做 IPC），一个专用线程把帧交给界面。
/// 队列满了丢**最旧**的一条，并合成一条 `events_dropped`：前端知道少了东西，
/// 而反压不会顺着 Python 的 stdout 写拖住整个 run。
pub struct EventQueue {
    state: Arc<QueueState>,
}

#[derive(Default)]
struct Slots {
    queue: VecDeque<Value>,
    dropped: u64,
    announced: u64,
    closed: bool,
}

struct QueueState {
    slots: Mutex<Slots>,
    wake: Condvar,
    capacity: usize,
}

impl EventQueue {
    /// 只建队列，不起转发线程（测试与"由调用方自己抽"的场合用）。
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(QueueState {
                slots: Mutex::new(Slots::default()),
                wake: Condvar::new(),
                capacity: capacity.max(1),
            }),
        })
    }

    /// 起队列与转发线程；`deliver` 由界面侧给出（Tauri 的 `Channel::send`）。
    pub fn start(capacity: usize, deliver: FrameSink) -> Arc<Self> {
        let queue = Self::new(capacity);
        let weak = Arc::downgrade(&queue);
        let _ = std::thread::Builder::new()
            .name("event-queue".to_owned())
            .spawn(move || {
                while let Some(queue) = weak.upgrade() {
                    let Some((warning, frame)) = queue.take() else {
                        return;
                    };
                    if let Some(warning) = warning {
                        deliver(warning);
                    }
                    deliver(frame);
                }
            });
        queue
    }

    /// 入队一条帧；满了先丢**最旧的装饰帧**（§4.5 `llm_delta`），再丢最旧的普通帧。
    ///
    /// token 级流式是装饰：少了它不影响任何结论、证据与预算，所以队列吃紧时
    /// 它先让位，而且**不计入 dropped**——那个计数是"界面少看了东西"的告警，
    /// 装饰帧不该拉响它（诊断里的数字仍然只数真丢的东西）。
    pub fn push(&self, frame: Value) {
        let Ok(mut slots) = self.state.slots.lock() else {
            return;
        };
        if slots.queue.len() >= self.state.capacity {
            match slots.queue.iter().position(is_decorative) {
                Some(index) => {
                    slots.queue.remove(index);
                }
                None => {
                    slots.queue.pop_front();
                    slots.dropped += 1;
                }
            }
        }
        slots.queue.push_back(frame);
        drop(slots);
        self.state.wake.notify_one();
    }

    /// 当前队列深度与累计丢弃数（诊断用）。
    pub fn stats(&self) -> (usize, u64) {
        self.state
            .slots
            .lock()
            .map(|slots| (slots.queue.len(), slots.dropped))
            .unwrap_or((0, 0))
    }

    fn close(&self) {
        if let Ok(mut slots) = self.state.slots.lock() {
            slots.closed = true;
        }
        self.state.wake.notify_all();
    }

    /// 取一帧；空了就等，直到有新帧或队列关闭。`None` = 关了且空了。
    /// 返回 `(告警, 帧)`：告警必须先于帧交给界面（"这里少了 N 条"）。
    fn take(&self) -> Option<(Option<Value>, Value)> {
        let mut slots = self.state.slots.lock().ok()?;
        loop {
            if let Some(frame) = slots.queue.pop_front() {
                let warning = if slots.dropped > slots.announced {
                    let dropped = slots.dropped - slots.announced;
                    slots.announced = slots.dropped;
                    Some(json!({
                        "type": "event",
                        "event": "events_dropped",
                        "run_id": "",
                        "seq": 0,
                        "ts_unix_ms": now_ms(),
                        "data": {"dropped": dropped, "capacity": self.state.capacity},
                    }))
                } else {
                    None
                };
                return Some((warning, frame));
            }
            if slots.closed {
                return None;
            }
            slots = self.state.wake.wait(slots).ok()?;
        }
    }
}

impl Drop for EventQueue {
    fn drop(&mut self) {
        self.close();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 装饰帧：丢了不影响任何结论、证据与预算的事件（§4.5 `llm_delta`）。
fn is_decorative(frame: &Value) -> bool {
    frame.get("event").and_then(Value::as_str) == Some("llm_delta")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_returns_short_lines_and_eof() {
        let mut reader = BufReader::new(&b"one\ntwo\n"[..]);
        let first = read_bounded_line(&mut reader, 64).unwrap().unwrap();
        assert_eq!(first, ("one".to_owned(), false));
        let second = read_bounded_line(&mut reader, 64).unwrap().unwrap();
        assert_eq!(second, ("two".to_owned(), false));
        assert!(read_bounded_line(&mut reader, 64).unwrap().is_none());
    }

    #[test]
    fn bounded_reader_drops_the_rest_of_an_oversized_line() {
        // 一行 40 字节 + 上限 16：只留下"超限"信号，后面那行必须还在。
        let mut raw = vec![b'x'; 40];
        raw.push(b'\n');
        raw.extend_from_slice(b"{\"id\":\"1\"}\n");
        let mut reader = BufReader::new(&raw[..]);
        let oversized = read_bounded_line(&mut reader, 16).unwrap().unwrap();
        assert!(oversized.1, "超长的一行必须被标记");
        let next = read_bounded_line(&mut reader, 16).unwrap().unwrap();
        assert_eq!(next, ("{\"id\":\"1\"}".to_owned(), false));
    }

    #[test]
    fn event_queue_drops_the_oldest_and_announces_it() {
        // 不起转发线程：6 条进 4 格，剩下的顺序是确定的（P8）。
        let queue = EventQueue::new(4);
        for index in 0..6 {
            queue.push(json!({"type": "event", "event": "tool_call_started", "index": index}));
        }
        let (warning, first) = queue.take().unwrap();
        let warning = warning.expect("丢了两条，必须有一条告警");
        assert_eq!(warning["event"], "events_dropped");
        assert_eq!(warning["data"]["dropped"], 2);
        assert_eq!(warning["data"]["capacity"], 4);
        assert_eq!(first["index"], 2, "丢的是最旧的两条");
        let rest: Vec<u64> = (0..3)
            .map(|_| {
                queue
                    .take()
                    .expect("队列里还有 3 条")
                    .1
                    .get("index")
                    .and_then(Value::as_u64)
                    .expect("index")
            })
            .collect();
        assert_eq!(rest, vec![3, 4, 5]);
        assert_eq!(queue.stats().1, 2);
    }

    #[test]
    fn event_queue_gives_up_streaming_frames_before_real_events() {
        // U11：4 格里已经有一条装饰帧，push 第 5 条时先走它，**不算丢弃、
        // 不发告警**；真事件一条不少，剩下的顺序也不乱。
        let queue = EventQueue::new(4);
        queue.push(json!({"type": "event", "event": "run_started", "index": 0}));
        queue.push(json!({"type": "event", "event": "llm_delta", "index": 1}));
        queue.push(json!({"type": "event", "event": "tool_call_started", "index": 2}));
        queue.push(json!({"type": "event", "event": "tool_call_finished", "index": 3}));
        queue.push(json!({"type": "event", "event": "llm_delta", "index": 4}));

        let (warning, first) = queue.take().unwrap();
        assert!(warning.is_none(), "装饰帧被丢不发 events_dropped");
        assert_eq!(first["index"], 0, "真事件一条都不该丢");
        assert_eq!(queue.stats().1, 0, "dropped 只数真丢的");

        let seen: Vec<u64> = (0..3)
            .map(|_| {
                queue
                    .take()
                    .expect("队列里还有 3 条")
                    .1
                    .get("index")
                    .and_then(Value::as_u64)
                    .expect("index")
            })
            .collect();
        assert_eq!(seen, vec![2, 3, 4], "顺序不变，只是旧的那条装饰帧先走了");
    }

    #[test]
    fn started_queue_delivers_on_its_own_thread() {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let deliver: FrameSink = Arc::new(move |frame: Value| {
            if let Ok(mut frames) = sink_seen.lock() {
                frames.push(frame);
            }
        });
        let queue = EventQueue::start(4, deliver);
        queue.push(json!({"type": "event", "event": "run_started"}));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "队列没有把帧交出来");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(seen.lock().unwrap()[0]["event"], "run_started");
        assert_eq!(queue.stats().0, 0);
    }
}
