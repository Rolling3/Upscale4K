//! Starting helper programs without a console window.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// A command for a console program (ffmpeg, ffprobe) that runs hidden:
/// no console window flashes up on Windows, and stdin is closed so the
/// program can never wait for keyboard input.
pub fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Start `command` so the program ends together with the app. On Windows a
/// started program normally keeps running when the app that started it is
/// ended from outside (End task, a crash), which would leave a hidden ffmpeg
/// encoding on its own. There it is put in a job that Windows closes, and so
/// ends, when the app's process ends in any way.
pub fn spawn_tied(command: &mut Command) -> io::Result<Child> {
    let child = command.spawn()?;
    #[cfg(windows)]
    windows_job::add(&child);
    Ok(child)
}

#[cfg(windows)]
mod windows_job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    /// The app never closes this handle; Windows closes it when the process ends.
    /// It is not inheritable, so the programs in the job don't keep it open.
    struct Job(HANDLE);

    // SAFETY: a job handle may be used from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls with valid arguments; the handle is checked before use.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    CloseHandle(handle);
                    return None;
                }
                Some(Job(handle))
            }
        })
        .as_ref()
    }

    /// Best effort: if Windows refuses, the program still runs, only not tied to the app.
    pub fn add(child: &Child) {
        if let Some(job) = job() {
            // SAFETY: both handles are valid for the duration of the call.
            unsafe {
                AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE);
            }
        }
    }
}

/// Most output kept from a short helper run, per stream.
const CAPTURE_LIMIT: usize = 8 * 1024 * 1024;

/// Result of a short helper run.
#[derive(Debug)]
pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `command` to completion, killing it if it takes longer than `timeout`.
/// Returns `None` when it couldn't start or was killed.
pub fn run_with_timeout(mut command: Command, timeout: Duration) -> Option<Captured> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn_tied(&mut command).ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    // ffprobe's JSON for a normal clip is a few kilobytes; a crafted file must not fill memory.
    let out_reader = thread::spawn(move || read_capped(&mut stdout, CAPTURE_LIMIT));
    let err_reader = thread::spawn(move || read_tail(&mut stderr, CAPTURE_LIMIT));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    status.map(|status| Captured { status, stdout, stderr })
}

/// Read at most `cap` bytes from `reader`, then keep draining the rest
/// without storing it, so the writer never blocks on a full pipe.
pub fn read_capped(reader: &mut impl Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let _ = reader.by_ref().take(cap as u64).read_to_end(&mut kept);
    let _ = std::io::copy(reader, &mut std::io::sink());
    kept
}

/// Read `reader` to the end but keep only its last `limit` bytes.
pub fn read_tail(reader: &mut impl Read, limit: usize) -> Vec<u8> {
    let mut tail: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                tail.extend_from_slice(&chunk[..n]);
                // Trim in batches so the buffer stays between `limit` and twice that.
                if tail.len() > limit.saturating_mul(2) {
                    tail.drain(..tail.len() - limit);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if tail.len() > limit {
        tail.drain(..tail.len() - limit);
    }
    tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn numbered(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn read_capped_keeps_the_start_and_consumes_everything() {
        let data = numbered(10_000);
        let mut reader = Cursor::new(data.clone());
        let got = read_capped(&mut reader, 4096);
        assert_eq!(got, data[..4096].to_vec());
        assert_eq!(reader.position(), 10_000, "the rest must still be drained");
    }

    #[test]
    fn read_capped_returns_short_input_whole() {
        assert_eq!(read_capped(&mut Cursor::new(b"abc".to_vec()), 4096), b"abc".to_vec());
    }

    #[test]
    fn read_tail_keeps_only_the_last_bytes() {
        let data = numbered(300_000);
        let got = read_tail(&mut Cursor::new(data.clone()), 65_536);
        assert_eq!(got.len(), 65_536);
        assert_eq!(got, data[data.len() - 65_536..].to_vec());
    }

    #[test]
    fn read_tail_returns_short_input_whole() {
        assert_eq!(read_tail(&mut Cursor::new(b"error line".to_vec()), 65_536), b"error line".to_vec());
    }
}
