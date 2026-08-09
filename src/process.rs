use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "windows")]
use std::os::windows::io::AsRawHandle;

#[derive(Debug)]
pub(crate) enum ProcessError {
    Spawn,
    Io,
    Timeout,
    OutputLimit,
}

pub(crate) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) fn run_bounded(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    output_limit: usize,
) -> Result<BoundedOutput, ProcessError> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(command);
    let mut child = command.spawn().map_err(|_| ProcessError::Spawn)?;
    let mut tree = ProcessTree::assign(&mut child)?;
    let (mut stdin, stdout, stderr) =
        match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
            (Some(stdin), Some(stdout), Some(stderr)) => (stdin, stdout, stderr),
            _ => {
                tree.terminate();
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProcessError::Io);
            }
        };
    let owned_input = input.to_vec();
    let (tx, rx) = mpsc::channel();
    let stdin_tx = tx.clone();
    let stdin_writer = thread::spawn(move || {
        let result = stdin.write_all(&owned_input).map_err(|_| ProcessError::Io);
        drop(stdin);
        let _ = stdin_tx.send((
            0_u8,
            result.as_ref().map(|_| Vec::new()).map_err(clone_error),
        ));
        result
    });
    let stdout_tx = tx.clone();
    let stdout_reader = thread::spawn(move || {
        let result = read_limited(stdout, output_limit);
        let _ = stdout_tx.send((1_u8, result.as_ref().cloned().map_err(clone_error)));
        result
    });
    let stderr_tx = tx;
    let stderr_reader = thread::spawn(move || {
        let result = read_limited(stderr, output_limit);
        let _ = stderr_tx.send((2_u8, result.as_ref().cloned().map_err(clone_error)));
        result
    });

    let deadline = Instant::now() + timeout;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let status = loop {
        while let Ok((kind, result)) = rx.try_recv() {
            match (kind, result) {
                (1, Ok(_)) => stdout_done = true,
                (2, Ok(_)) => stderr_done = true,
                (_, Err(error)) => {
                    tree.terminate();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
                _ => {}
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => {
                tree.terminate();
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProcessError::Io);
            }
        }
        if Instant::now() >= deadline {
            tree.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProcessError::Timeout);
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !stdout_done || !stderr_done {
        // The process has exited; pipe readers now have a finite EOF.
    }
    stdin_writer.join().map_err(|_| ProcessError::Io)??;
    let stdout = stdout_reader.join().map_err(|_| ProcessError::Io)??;
    let stderr = stderr_reader.join().map_err(|_| ProcessError::Io)??;
    tree.disarm();
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

fn clone_error(error: &ProcessError) -> ProcessError {
    match error {
        ProcessError::Spawn => ProcessError::Spawn,
        ProcessError::Io => ProcessError::Io,
        ProcessError::Timeout => ProcessError::Timeout,
        ProcessError::OutputLimit => ProcessError::OutputLimit,
    }
}

fn read_limited(mut pipe: impl Read, limit: usize) -> Result<Vec<u8>, ProcessError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = pipe.read(&mut buffer).map_err(|_| ProcessError::Io)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > limit {
            return Err(ProcessError::OutputLimit);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

struct ProcessTree {
    #[cfg(target_os = "windows")]
    job: *mut core::ffi::c_void,
    #[cfg(unix)]
    process_group: i32,
    active: bool,
}

impl ProcessTree {
    fn assign(child: &mut std::process::Child) -> Result<Self, ProcessError> {
        #[cfg(target_os = "windows")]
        {
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() || unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0
            {
                if !job.is_null() {
                    unsafe { CloseHandle(job) };
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProcessError::Spawn);
            }
            Ok(Self { job, active: true })
        }
        #[cfg(unix)]
        {
            Ok(Self {
                process_group: child.id() as i32,
                active: true,
            })
        }
        #[cfg(not(any(target_os = "windows", unix)))]
        {
            let _ = child;
            Ok(Self { active: true })
        }
    }

    fn terminate(&self) {
        #[cfg(target_os = "windows")]
        unsafe {
            TerminateJobObject(self.job, 1);
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.process_group, libc::SIGKILL);
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if self.active {
            self.terminate();
        }
        #[cfg(target_os = "windows")]
        unsafe {
            CloseHandle(self.job);
        }
    }
}

#[cfg(target_os = "windows")]
unsafe impl Send for ProcessTree {}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(
        job_attributes: *const core::ffi::c_void,
        name: *const u16,
    ) -> *mut core::ffi::c_void;
    fn AssignProcessToJobObject(
        job: *mut core::ffi::c_void,
        process: *mut core::ffi::c_void,
    ) -> i32;
    fn TerminateJobObject(job: *mut core::ffi::c_void, exit_code: u32) -> i32;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_reader_enforces_exact_limit() {
        assert_eq!(read_limited(&b"abcd"[..], 4).unwrap(), b"abcd");
        assert!(matches!(
            read_limited(&b"abcde"[..], 4),
            Err(ProcessError::OutputLimit)
        ));
    }

    #[test]
    fn bounded_child_is_terminated_on_deadline() {
        if std::env::var_os("VMCELL_BOUNDED_CHILD").is_some() {
            thread::sleep(Duration::from_secs(30));
            return;
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("process::tests::bounded_child_is_terminated_on_deadline")
            .arg("--nocapture")
            .env("VMCELL_BOUNDED_CHILD", "1");
        let started = Instant::now();
        assert!(matches!(
            run_bounded(&mut command, &[], Duration::from_millis(250), 1024),
            Err(ProcessError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
