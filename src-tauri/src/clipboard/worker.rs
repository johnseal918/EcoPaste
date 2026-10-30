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

/// An unsuccessful native read is a valid protocol reply. Only framing/pipe
/// failure terminates the response reader and invalidates the process.
enum WorkerReply {
    Captured(Option<ClipboardPayload>),
    ReadFailed(String),
}

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

fn read_reply(input: &mut impl Read) -> io::Result<WorkerReply> {
    let mut header = [0u8; 17];
    input.read_exact(&mut header)?;
    let kind = header[0];
    let width = u32::from_le_bytes(header[1..5].try_into().unwrap());
    let height = u32::from_le_bytes(header[5..9].try_into().unwrap());
    let size = u64::from_le_bytes(header[9..17].try_into().unwrap());
    if size > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "clipboard worker response exceeded capacity",
        ));
    }
    let mut bytes = vec![0u8; size as usize];
    input.read_exact(&mut bytes)?;
    let decode = |error: serde_json::Error| io::Error::new(io::ErrorKind::InvalidData, error);
    match kind {
        0 if size == 0 => Ok(WorkerReply::Captured(None)),
        1 => serde_json::from_slice::<TextPayload>(&bytes)
            .map(|value| WorkerReply::Captured(Some(ClipboardPayload::Text(value))))
            .map_err(decode),
        2 if width != 0 && height != 0 => {
            Ok(WorkerReply::Captured(Some(ClipboardPayload::Image(
                ImagePayload { bytes, width, height },
            ))))
        }
        3 => serde_json::from_slice::<Vec<String>>(&bytes)
            .map(|value| WorkerReply::Captured(Some(ClipboardPayload::Files(value))))
            .map_err(decode),
        4 => Ok(WorkerReply::ReadFailed(
            String::from_utf8_lossy(&bytes).into_owned(),
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "clipboard worker returned invalid frame",
        )),
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
    replies: Receiver<io::Result<WorkerReply>>,
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
                // A read failure is a valid WorkerReply::ReadFailed, so keep
                // this pipe reader alive. Only an actual transport error ends it.
                let response = read_reply(&mut stdout);
                let failed_transport = response.is_err();
                if tx.send(response).is_err() || failed_transport {
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

impl WorkerProcess {
    /// Request termination of only this verified child PID. TerminateProcess
    /// is asynchronous: never wait()/join() on the foreground capture path.
    fn request_stop(&mut self) {
        match self.child.kill() {
            Ok(()) => log::info!("clipboard helper stop requested pid={}", self.child.id()),
            Err(err) => log::warn!(
                "clipboard helper stop request failed pid={} error={err}",
                self.child.id()
            ),
        }
    }

    fn poll_exit(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                log::info!(
                    "clipboard helper exited pid={} status={status}",
                    self.child.id()
                );
                true
            }
            Ok(None) => false,
            Err(err) => {
                log::warn!(
                    "clipboard helper exit check failed pid={} error={err}",
                    self.child.id()
                );
                false
            }
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        // Drop may run on the Tauri request thread. Never block it with
        // wait()/join(); close the child handles and detach the pipe readers.
        // Normal and abnormal process exits are handled by try_wait in the
        // session; a final nonblocking stop request protects normal shutdown.
        if !self.poll_exit() {
            self.request_stop();
        }
        self.reply_reader.take();
        self.diagnostic_reader.take();
    }
}

/// A child which failed to exit is quarantined rather than replaced by more
/// helpers. At most one owned process can be active or waiting for retirement.
#[derive(Default)]
pub struct WorkerSession {
    process: Option<WorkerProcess>,
    retiring: Option<WorkerProcess>,
    blocked_sequence: Option<u32>,
}

impl WorkerSession {
    /// Termination is requested once and observed with try_wait on subsequent
    /// watcher iterations. Nothing waits for another process on this thread.
    fn retire_current(&mut self, sequence: u32, reason: &'static str) {
        self.blocked_sequence = Some(sequence);
        if let Some(mut worker) = self.process.take() {
            log::warn!(
                "clipboard helper retiring pid={} sequence={sequence} reason={reason}",
                worker.child.id()
            );
            worker.request_stop();
            self.retiring = Some(worker);
        }
    }

    fn retirement_finished(&mut self) -> bool {
        let Some(worker) = self.retiring.as_mut() else {
            return true;
        };
        if !worker.poll_exit() {
            return false;
        }
        // Releasing pipe handles and JoinHandle references cannot block here.
        self.retiring.take();
        true
    }

    pub fn capture(&mut self, capture: &Capture, age: Duration) -> Result<Option<ClipboardPayload>> {
        let sequence = unsafe { winapi::um::winuser::GetClipboardSequenceNumber() };
        if super::windows_write_pending() {
            return Err(AppError::Clipboard(
                "clipboard worker yields to foreground write".to_owned(),
            ));
        }
        if !self.retirement_finished() {
            return Err(AppError::Clipboard(
                "clipboard helper retirement still pending; no new worker started".to_owned(),
            ));
        }
        if self.blocked_sequence == Some(sequence) {
            return Err(AppError::Clipboard(
                "clipboard helper transport for this sequence is quarantined".to_owned(),
            ));
        }
        if self.blocked_sequence.is_some() {
            self.blocked_sequence = None;
        }
        if self.process.is_none() {
            self.process = Some(WorkerProcess::spawn()?);
        }
        let request = Request {
            capture: capture.clone(),
            age_ms: age.as_millis().min(u64::MAX as u128) as u64,
            expected_sequence: sequence,
        };
        if let Err(err) = self.process.as_mut().unwrap().submit(&request) {
            self.retire_current(sequence, "request_pipe_failed");
            return Err(err);
        }
        let started = Instant::now();
        loop {
            if super::windows_write_pending() {
                log::warn!(
                    "clipboard worker preempted for foreground paste after_ms={}",
                    started.elapsed().as_millis()
                );
                self.retire_current(sequence, "foreground_write");
                return Err(AppError::Clipboard(
                    "clipboard worker canceled for foreground write".to_owned(),
                ));
            }
            if started.elapsed() >= MAX_BACKGROUND_CAPTURE {
                log::warn!(
                    "clipboard worker native call timed out after_ms={}",
                    started.elapsed().as_millis()
                );
                self.retire_current(sequence, "background_timeout");
                return Err(AppError::Clipboard(
                    "clipboard worker capture timed out and retirement requested".to_owned(),
                ));
            }
            match self.process.as_ref().unwrap().replies.recv_timeout(RESPONSE_POLL) {
                Ok(Ok(WorkerReply::Captured(payload))) => return Ok(payload),
                Ok(Ok(WorkerReply::ReadFailed(error))) => {
                    // A normal OS/business failure does not destroy the pipe.
                    return Err(AppError::Clipboard(error));
                }
                Ok(Err(error)) => {
                    self.retire_current(sequence, "response_protocol_failed");
                    return Err(AppError::Clipboard(format!(
                        "clipboard helper response protocol failed: {error}"
                    )));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.retire_current(sequence, "response_channel_closed");
                    return Err(AppError::Clipboard(
                        "clipboard helper disconnected during capture".to_owned(),
                    ));
                }
            }
        }
    }
}
