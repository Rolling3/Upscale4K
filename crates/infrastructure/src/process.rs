//! Starting helper programs without a console window.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::ops::{Deref, DerefMut};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A command for a console program (ffmpeg, ffprobe) that runs hidden:
/// no console window flashes up on Windows, and stdin is closed so the
/// program can never wait for keyboard input.
pub fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A helper program started with [`spawn_tied`]. It derefs to the
/// [`Child`], except that [`TiedChild::kill`] also ends what it started.
pub struct TiedChild {
    child: Child,
    #[cfg(windows)]
    job: Option<windows_job::Job>,
}

impl TiedChild {
    /// End the program and, on Windows, every program it started.
    pub fn kill(&mut self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
        let _ = self.child.kill();
    }
}

impl Deref for TiedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}

impl DerefMut for TiedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

/// Start a command made by [`hidden_command`] so that the program, and
/// anything it starts, ends together with it.
///
/// On Windows a started program normally outlives whatever started it. Two
/// things go wrong without a job: Chocolatey's `ffmpeg.exe` is a launcher
/// that runs the real ffmpeg as a second process, so stopping the launcher
/// on cancel left the real encode running; and when the app was ended from
/// outside (End task, a crash) its ffmpeg kept encoding on its own. Each
/// program therefore runs in its own job: [`TiedChild::kill`] ends the whole
/// job, and Windows ends it when the job's last handle closes, which also
/// happens when the app's process ends in any way. The program is started
/// paused and only let run once it is in the job, so nothing it launches can
/// slip out.
pub fn spawn_tied(command: &mut Command) -> io::Result<TiedChild> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let mut child = command.spawn()?;
        // If Windows refuses a job, the program still runs, only without these guarantees.
        let job = windows_job::Job::new().filter(|job| job.add(&child));
        if !windows_job::resume(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("the program was started but could not be let run"));
        }
        Ok(TiedChild { child, job })
    }
    #[cfg(not(windows))]
    {
        Ok(TiedChild { child: command.spawn()? })
    }
}

#[cfg(windows)]
mod windows_job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
        TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    /// A job that ends every program in it when its last handle closes. The
    /// handle is not inheritable, so the programs in the job don't hold it.
    pub struct Job(HANDLE);

    // SAFETY: a job handle may be used from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Option<Job> {
            // SAFETY: plain Win32 calls with valid arguments; the handle is checked before use.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Job(handle);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                (set != 0).then_some(job)
            }
        }

        pub fn add(&self, child: &Child) -> bool {
            // SAFETY: both handles are valid for the duration of the call.
            unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
        }

        pub fn terminate(&self) {
            // SAFETY: the handle is valid until drop.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// Let a process that was started paused run: resume its threads.
    pub fn resume(process_id: u32) -> bool {
        // SAFETY: plain Win32 calls; every handle opened here is closed here.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut resumed = false;
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == process_id {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        resumed |= ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            resumed
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
                child.kill();
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
