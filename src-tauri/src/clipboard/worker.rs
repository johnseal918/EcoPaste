//! Windows-only, out-of-process clipboard capture.
//!
//! GetClipboardData can synchronously wait for WM_RENDERFORMAT for up to
//! 30 seconds. Neither a Rust thread timeout nor a process-local Mutex
//! can cancel that native call. A persistent helper process isolates the
//! blocked Win32 transaction; the application can retire this helper when
//! a foreground paste begins or a bounded background budget expires.
//!
//! Clipboard content travels through anonymous inherited pipes only.
//! The helper does not initialize Tauri, the database or user settings.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::payload::{ClipboardPayload, ImagePayload, TextPayload};
use super::read::ClipboardReader;
use crate::core::{AppError, Result};
use crate::settings::Capture;

const WORKER_SWITCH: &str = "--ecopaste-internal-clipboard-capture";
const MAX_FRAME_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BACKGROUND_CAPTURE: Duration = Duration::from_secs(8);
const RESPONSE_POLL: Duration = Duration::from_millis(10);

#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    capture: Capture,
    age_ms: u64,
    expected_sequence: u32,
}

/// Text/files are encoded as JSON; PNG data is passed unchanged, not
/// JSON-encoded or saved to a temporary file.
fn write_reply(out: &mut impl Write, result: Result<Option<ClipboardPayload>>) -> io::Result<()> {
    let (kind, width, height, bytes) = match result {
        Ok(None) => (0u8, 0, 0, Vec::new()),
        Ok(Some(ClipboardPayload::Text(value))) => (
            1, 0, 0,
            serde_json::to_vec(&value).map_err(io::Error::other)?,
        ),
        Ok(Some(ClipboardPayload::Image(value))) => (
            2, value.width, value.height, value.bytes,
        ),
        Ok(Some(ClipboardPayload::Files(value))) => (
            3, 0, 0,
            serde_json::to_vec(&value).map_err(io::Error::other)?,
        ),
        Err(err) => (4, 0, 0, err.to_string().into_bytes()),
    };
    let size = bytes.len() as u64;
    if size > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "clipboard worker response exceeds bounded pipe capacity",
        ));
    }
    out.write_all(&[kind])?;
    out.write_all(&width.to_le_bytes())?;
    out.write_all(&height.to_le_bytes())?;
    out.write_all(&size.to_le_bytes())?;
    out.write_all(&bytes)?;
    out.flush()
}

fn read_reply(input: &mut impl Read) -> std::result::Result<Option<ClipboardPayload>, String> {
    let mut header = [0u8; 17];
    input.read_exact(&mut header).map_err(|err| err.to_string())?;
    let kind = header[0];
    let width = u32::from_le_bytes(header[1..5].try_into().unwrap());
    let height = u32::from_le_bytes(header[5..9].try_into().unwrap());
    let size = u64::from_le_bytes(header[9..17].try_into().unwrap());
    if size > MAX_FRAME_BYTES {
        return Err("clipboard worker response exceeded capacity".to_owned());
    }
    let mut bytes = vec![0u8; size as usize];
    input.read_exact(&mut bytes).map_err(|err| err.to_string())?;
    match kind {
        0 => Ok(None),
        1 => serde_json::from_slice::<TextPayload>(&bytes)
            .map(ClipboardPayload::Text)
            .map(Some)
            .map_err(|err| err.to_string()),
        2 => Ok(Some(ClipboardPayload::Image(ImagePayload { bytes, width, height }))),
        3 => serde_json::from_slice::<Vec<String>>(&bytes)
            .map(ClipboardPayload::Files)
            .map(Some)
            .map_err(|err| err.to_string()),
        4 => Err(String::from_utf8_lossy(&bytes).into_owned()),
        _ => Err("clipboard worker returned unknown payload kind".to_owned()),
    }
}

struct WorkerLog;

impl log::Log for WorkerLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
            && metadata.target().starts_with("eco_paste_lib::clipboard")
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            // The caller only forwards metadata lines; never print content.
            eprintln!("[{}] {}", record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

static WORKER_LOG: WorkerLog = WorkerLog;

/// Called before the GUI bootstrap and single-instance plugin. A worker has
/// neither tray/UI nor database access, and exits when the stdin pipe closes.
pub fn run_if_requested() -> bool {
    if std::env::args_os().nth(1).as_deref()
        != Some(std::ffi::OsStr::new(WORKER_SWITCH))
    {
        return false;
    }
    let _ = log::set_logger(&WORKER_LOG);
    log::set_max_level(log::LevelFilter::Info);
    let input = io::stdin();
    let output = io::stdout();
    let mut out = output.lock();
    let reader = ClipboardReader::new();
    for line in input.lock().lines() {
        let Ok(line) = line else { break };
        let result = match serde_json::from_str::<Request>(&line) {
            Err(err) => Err(AppError::Clipboard(format!(
                "invalid capture worker request: {err}"
            ))),
            Ok(request) => match &reader {
                Err(err) => Err(AppError::Clipboard(err.to_string())),
                Ok(reader) => {
                    let before = unsafe {
                        winapi::um::winuser::GetClipboardSequenceNumber()
                    };
                    if request.expected_sequence != 0 && before != request.expected_sequence {
                        Err(AppError::Clipboard(
                            "clipboard worker found a stale sequence before capture".to_owned(),
                        ))
                    } else {
                        reader.read_native_with_capture_age(
                            &request.capture,
                            Duration::from_millis(request.age_ms),
                        )
                    }
                }
            },
        };
        if write_reply(&mut out, result).is_err() {
            break;
        }
    }
    true
}

struct WorkerProcess {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<std::result::Result<Option<ClipboardPayload>, String>>,
    reply_reader: Option<JoinHandle<()>>,
    diagnostic_reader: Option<JoinHandle<()>>,
}

impl WorkerProcess {
    fn spawn() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|err| {
            AppError::Clipboard(format!("clipboard helper executable unavailable: {err}"))
        })?;
        let mut child = Command::new(exe)
            .arg(WORKER_SWITCH)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                AppError::Clipboard(format!("clipboard helper could not start: {err}"))
            })?;
        let stdin = child.stdin.take().expect("worker stdin was piped");
        let mut stdout = child.stdout.take().expect("worker stdout was piped");
        let stderr = child.stderr.take().expect("worker stderr was piped");
        let (tx, rx) = mpsc::channel();
        let reply_reader = std::thread::Builder::new()
            .name("clipboard-worker-pipe".to_owned())
            .spawn(move || loop {
                let response = read_reply(&mut stdout);
                let closed = response.is_err();
                if tx.send(response).is_err() || closed {
                    break;
                }
            })
            .expect("spawn clipboard response reader");
        let diagnostic_reader = std::thread::Builder::new()
            .name("clipboard-worker-diagnostics".to_owned())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    // Only metadata emitted by our internal clipboard logger.
                    if line.len() <= 1024
                        && (line.contains("native_read")
                            || line.contains("clipboard"))
                    {
                        log::info!("clipboard helper {line}");
                    }
                }
            })
            .expect("spawn clipboard diagnostics reader");
        log::info!("clipboard worker started pid={}", child.id());
        Ok(Self {
            child,
            stdin,
            replies: rx,
            reply_reader: Some(reply_reader),
            diagnostic_reader: Some(diagnostic_reader),
        })
    }

    fn submit(&mut self, request: &Request) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, request).map_err(|err| {
            AppError::Clipboard(format!("clipboard worker request encoding: {err}"))
        })?;
        self.stdin.write_all(b"\n").map_err(|err| {
            AppError::Clipboard(format!("clipboard worker request pipe closed: {err}"))
        })?;
        self.stdin.flush().map_err(|err| {
            AppError::Clipboard(format!("clipboard worker request flush failed: {err}"))
        })
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        // Only the EcoPaste-owned worker is terminated. Win32 releases a
        // terminated process's open clipboard; never kill a source app.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.reply_reader.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.diagnostic_reader.take() {
            let _ = handle.join();
        }
    }
}

/// Persistent for ordinary rapid copies; recreated after timeout, crash or
/// foreground preemption. The only watcher remains in the main process.
#[derive(Default)]
pub struct WorkerSession {
    process: Option<WorkerProcess>,
}

impl WorkerSession {
    pub fn capture(&mut self, capture: &Capture, age: Duration) -> Result<Option<ClipboardPayload>> {
        if super::windows_write_pending() {
            return Err(AppError::Clipboard(
                "clipboard worker yields to foreground write".to_owned(),
            ));
        }
        if self.process.is_none() {
            self.process = Some(WorkerProcess::spawn()?);
        }
        let request = Request {
            capture: capture.clone(),
            age_ms: age.as_millis().min(u64::MAX as u128) as u64,
            expected_sequence: unsafe {
                winapi::um::winuser::GetClipboardSequenceNumber()
            },
        };
        if let Err(err) = self.process.as_mut().unwrap().submit(&request) {
            self.process = None;
            return Err(err);
        }
        let started = Instant::now();
        loop {
            if super::windows_write_pending() {
                log::warn!(
                    "clipboard worker preempted for foreground paste after_ms={}",
                    started.elapsed().as_millis()
                );
                self.process = None;
                return Err(AppError::Clipboard(
                    "clipboard worker canceled for foreground write".to_owned(),
                ));
            }
            if started.elapsed() >= MAX_BACKGROUND_CAPTURE {
                log::warn!(
                    "clipboard worker native call timed out after_ms={}",
                    started.elapsed().as_millis()
                );
                self.process = None;
                return Err(AppError::Clipboard(
                    "clipboard worker capture timed out and was retired".to_owned(),
                ));
            }
            match self.process.as_ref().unwrap().replies.recv_timeout(RESPONSE_POLL) {
                Ok(Ok(payload)) => return Ok(payload),
                Ok(Err(error)) => {
                    // Native errors may be transient, but a dead pipe must
                    // always get a clean worker on the next clipboard event.
                    if self.process.as_mut().is_some_and(|p| {
                        p.child.try_wait().ok().flatten().is_some()
                    }) {
                        self.process = None;
                    }
                    return Err(AppError::Clipboard(error));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.process = None;
                    return Err(AppError::Clipboard(
                        "clipboard helper disconnected during capture".to_owned(),
                    ));
                }
            }
        }
    }
}
