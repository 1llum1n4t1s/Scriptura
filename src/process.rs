use std::{
    io::{self, Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;

pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
pub fn command(program: &str) -> Command {
    // `mut` は Windows の `creation_flags` 呼び出しにだけ必要。
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}
// stdin/stdout/stderr と直接子の終了へ同じ期限を適用する。
pub fn capture(
    mut cmd: Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    managed: bool,
) -> io::Result<Output> {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(if managed {
        Stdio::null()
    } else {
        Stdio::piped()
    })
    .stderr(if managed {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(if managed { 0x08000004 } else { 0x08000000 });
    }
    #[cfg(windows)]
    let mut job = if managed { Some(Job::new()?) } else { None };
    let deadline = Instant::now() + timeout;
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    if let Some(job) = &job
        && let Err(e) = job.attach(&child)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let out = child.stdout.take();
    let diagnostic = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    let out_tx = tx.clone();
    let reader = thread::spawn(move || {
        let mut v = Vec::new();
        let result = if let Some(mut out) = out {
            out.read_to_end(&mut v).map(|_| v)
        } else {
            Ok(v)
        };
        let _ = out_tx.send((0, result));
    });
    let err_tx = tx.clone();
    let diagnostic = thread::spawn(move || {
        let mut v = Vec::new();
        let mut buffer = [0u8; 8192];
        let result = (|| -> io::Result<Vec<u8>> {
            if let Some(mut err) = diagnostic {
                loop {
                    let n = err.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    v.extend_from_slice(&buffer[..n]);
                    if managed && v.len() > 8192 {
                        v.drain(..v.len() - 8192);
                    }
                }
            }
            Ok(v)
        })();
        let _ = err_tx.send((1, result));
    });
    let writer_tx = tx.clone();
    let writer = input.map(|v| {
        let mut stdin = child.stdin.take().unwrap();
        thread::spawn(move || {
            let result = stdin.write_all(&v);
            drop(stdin);
            let result = match result {
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(vec![]),
                r => r.map(|_| vec![]),
            };
            let _ = writer_tx.send((2, result));
        })
    });
    drop(tx);
    let count = 2 + usize::from(writer.is_some());
    let mut completed = 0;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let work = (|| -> io::Result<std::process::ExitStatus> {
        let status = child
            .wait_timeout(deadline.saturating_duration_since(Instant::now()))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "process timeout"))?;
        #[cfg(windows)]
        if managed {
            if let Some(job) = &job {
                job.terminate();
                if !job.wait_empty(deadline) {
                    return Err(io::Error::other("process termination unconfirmed"));
                }
            }
            drop(job.take());
        }
        while completed < count {
            let (kind, result) = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "process pipe timeout"))?;
            completed += 1;
            let bytes = result?;
            match kind {
                0 => stdout = bytes,
                1 => stderr = bytes,
                _ => {}
            }
        }
        Ok(status)
    })();
    let status = match work {
        Ok(status) => status,
        Err(error) => {
            if !managed {
                // 正常な Git/SSH helper を終了させず、結果不明として呼出元へ戻す。
                // 残る pipe thread は、この単発 hook process の終了で解放する。
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                }
                return Err(io::Error::other("process result unconfirmed"));
            }
            #[cfg(windows)]
            if let Some(job) = &job {
                job.terminate();
            }
            let _ = child.kill();
            let teardown = Instant::now() + Duration::from_secs(15);
            let reaped = child
                .wait_timeout(Duration::from_secs(15))
                .ok()
                .flatten()
                .is_some();
            while completed < count {
                match rx.recv_timeout(teardown.saturating_duration_since(Instant::now())) {
                    Ok(_) => completed += 1,
                    Err(_) => break,
                }
            }
            #[cfg(windows)]
            let empty = job.as_ref().is_none_or(|j| j.wait_empty(teardown));
            #[cfg(not(windows))]
            let empty = true;
            if !reaped || completed < count || !empty {
                return Err(io::Error::other("process termination unconfirmed"));
            }
            let _ = reader.join();
            let _ = diagnostic.join();
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            return Err(error);
        }
    };
    let _ = reader.join();
    let _ = diagnostic.join();
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    Ok(Output {
        code: status.code(),
        stdout,
        stderr,
    })
}

#[cfg(windows)]
pub struct Job(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Job {
    pub fn new() -> io::Result<Self> {
        unsafe {
            use windows_sys::Win32::System::JobObjects::*;
            let job = Self(CreateJobObjectW(std::ptr::null(), std::ptr::null()));
            if job.0.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut limit: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limit as *const _ as *const _,
                std::mem::size_of_val(&limit) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }
    pub fn attach(&self, child: &std::process::Child) -> io::Result<()> {
        unsafe {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
            #[link(name = "ntdll")]
            unsafe extern "system" {
                fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
            }
            if AssignProcessToJobObject(self.0, child.as_raw_handle()) == 0 {
                return Err(io::Error::last_os_error());
            }
            if NtResumeProcess(child.as_raw_handle()) != 0 {
                return Err(io::Error::other("worker resume failed"));
            }
            Ok(())
        }
    }
    pub fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
    pub fn wait_empty(&self, deadline: Instant) -> bool {
        unsafe {
            use windows_sys::Win32::System::JobObjects::*;
            loop {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                if QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return false;
                }
                if info.ActiveProcesses == 0 {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
