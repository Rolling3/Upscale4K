//! Starting helper programs without a console window.

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
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
    let mut child = command.spawn().ok()?;
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
