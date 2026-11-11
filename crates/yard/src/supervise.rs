//! How task phases run. One trait, three backends:
//!
//! - **systemd** (Linux with a user manager): transient units, cgroup limits
//!   enforced by the kernel, usage read from the unit's cgroup.
//! - **process** (macOS, Linux without systemd): a session of its own per
//!   phase, background priority, usage summed over the process tree, and a
//!   memory cap enforced by the daemon (the tree is killed when it passes it).
//! - **job** (Windows): a named Job Object with a hard memory limit and idle
//!   priority class; usage from job accounting.
//!
//! Every backend leaves tasks running if the daemon exits, and can pick them
//! up again from the [`Handle`] saved in the runs file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Handle {
    /// Unit or job name.
    pub name: String,
    /// Root process (process and job backends).
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub mem_cap: Option<u64>,
}

#[allow(dead_code)]
pub struct Spawn<'a> {
    pub name: String,
    pub description: String,
    pub dir: &'a Path,
    pub env: &'a BTreeMap<String, String>,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub mem: Option<u64>,
    pub idle: bool,
    pub nice: i32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub alive: bool,
    pub mem: Option<u64>,
    pub cpu: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Ended {
    /// Nothing more is known; use the exit file or assume a failure.
    Unknown,
    Code(i32),
    /// Killed for passing its memory cap.
    Oom,
}

pub trait Supervisor {
    fn name(&self) -> &'static str;
    fn spawn(&mut self, s: &Spawn) -> Result<Handle, String>;
    /// Liveness and resource use, called about once a second per run.
    fn poll(&mut self, h: &Handle) -> Usage;
    fn stop(&mut self, h: &Handle);
    /// Why a run vanished without writing its exit file.
    fn post_mortem(&mut self, h: &Handle) -> Ended;
    /// The run is over; drop anything kept for it.
    fn forget(&mut self, h: &Handle, failed: bool);
}

/// Picks the backend: `SWITCHYARD_BACKEND` if set, else the platform's best.
pub fn pick() -> Box<dyn Supervisor> {
    let want = std::env::var("SWITCHYARD_BACKEND").unwrap_or_default();
    #[cfg(target_os = "linux")]
    {
        if want != "process" && systemd::available() {
            return Box::new(systemd::Systemd::default());
        }
    }
    #[cfg(unix)]
    {
        let _ = &want;
        Box::new(process::Process::new())
    }
    #[cfg(windows)]
    {
        let _ = &want;
        Box::new(job::Job::new())
    }
}

/// Finds a program on PATH (with PATHEXT on Windows).
pub fn which(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.exists().then(|| p.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|e| e.to_lowercase())
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        for e in &exts {
            let c = dir.join(format!("{name}{e}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// The shell steps run in, when the config does not say.
pub fn default_shell() -> Vec<String> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if cfg!(windows) {
        if which("pwsh").is_some() {
            v(&["pwsh", "-NoLogo", "-NoProfile", "-Command"])
        } else if which("powershell").is_some() {
            v(&["powershell", "-NoLogo", "-NoProfile", "-Command"])
        } else {
            v(&["cmd", "/C"])
        }
    } else if which("bash").is_some() {
        v(&["bash", "-c"])
    } else {
        v(&["sh", "-c"])
    }
}

// ---------------------------------------------------------------- systemd

#[cfg(target_os = "linux")]
pub mod systemd {
    use super::*;
    use crate::sys;
    use std::collections::HashMap;
    use std::process::Command;
    use std::time::Instant;

    pub fn available() -> bool {
        super::which("systemd-run").is_some()
            && Command::new("systemctl")
                .args(["--user", "show-environment"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    }

    #[derive(Default)]
    struct State {
        cgroup: Option<PathBuf>,
        tries: u32,
        cpu_prev: Option<(u64, Instant)>,
    }

    #[derive(Default)]
    pub struct Systemd {
        state: HashMap<String, State>,
    }

    impl Supervisor for Systemd {
        fn name(&self) -> &'static str {
            "systemd"
        }

        fn spawn(&mut self, s: &Spawn) -> Result<Handle, String> {
            let mut c = Command::new("systemd-run");
            c.args(["--user", "--quiet", "--collect"]);
            c.arg(format!("--unit={}", s.name));
            c.arg(format!("--description={}", s.description));
            c.arg(format!("--working-directory={}", s.dir.display()));
            c.args(["-p", "KillMode=control-group", "-p", "TimeoutStopSec=10"]);
            if let Some(m) = s.mem {
                c.args(["-p", &format!("MemoryMax={}", yard_core::size::systemd(m)), "-p", "MemorySwapMax=0"]);
            }
            if s.idle {
                c.args(["-p", "CPUWeight=idle", "-p", &format!("Nice={}", s.nice)]);
            }
            for (k, v) in s.env {
                c.arg(format!("--setenv={k}={v}"));
            }
            c.arg("--").arg(&s.program).args(&s.args);
            c.stdin(std::process::Stdio::null());
            match c.output() {
                Ok(o) if o.status.success() => {
                    self.state.insert(s.name.clone(), State::default());
                    Ok(Handle { name: s.name.clone(), pid: None, mem_cap: s.mem })
                }
                Ok(o) => Err(format!("systemd-run failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
                Err(e) => Err(format!("could not run systemd-run: {e}")),
            }
        }

        fn poll(&mut self, h: &Handle) -> Usage {
            let st = self.state.entry(h.name.clone()).or_default();
            if st.cgroup.is_none() && st.tries < 4 {
                st.tries += 1;
                st.cgroup = sys::cgroup_path(&h.name);
            }
            let Some(cg) = st.cgroup.clone() else { return Usage::default() };
            if !sys::cgroup_alive(&cg) {
                return Usage::default();
            }
            let mem = sys::read_u64(&cg.join("memory.current"));
            let mut cpu = None;
            if let Some(u) = sys::cpu_usage_usec(&cg) {
                let now = Instant::now();
                if let Some((pu, pt)) = st.cpu_prev {
                    let wall = now.duration_since(pt).as_micros().max(1) as f64;
                    cpu = Some((u.saturating_sub(pu) as f64 / wall * 100.0) as f32);
                }
                st.cpu_prev = Some((u, now));
            }
            Usage { alive: true, mem, cpu }
        }

        fn stop(&mut self, h: &Handle) {
            let mut c = Command::new("systemctl");
            c.args(["--user", "stop", "--no-block", &h.name]);
            let _ = sys::spawn_detached(c);
        }

        fn post_mortem(&mut self, h: &Handle) -> Ended {
            let props = sys::unit_properties(&h.name, &["ActiveState", "Result", "ExecMainStatus"]);
            let state = props.get("ActiveState").map(String::as_str).unwrap_or("");
            if matches!(state, "active" | "activating" | "reloading") {
                // Still there: the cgroup lookup was wrong; try again.
                if let Some(st) = self.state.get_mut(&h.name) {
                    st.cgroup = None;
                    st.tries = 0;
                }
                return Ended::Code(-1);
            }
            if props.get("Result").is_some_and(|r| r == "oom-kill") {
                return Ended::Oom;
            }
            match props.get("ExecMainStatus").and_then(|s| s.parse::<i32>().ok()) {
                Some(c) if c != 0 => Ended::Code(c),
                _ => Ended::Unknown,
            }
        }

        fn forget(&mut self, h: &Handle, failed: bool) {
            self.state.remove(&h.name);
            if failed {
                let mut c = Command::new("systemctl");
                c.args(["--user", "reset-failed", &h.name]);
                let _ = sys::spawn_detached(c);
            }
        }
    }
}

// ---------------------------------------------------------------- process

/// Sums memory and CPU over a process and all its descendants.
struct Tree {
    sys: Option<sysinfo::System>,
}

impl Tree {
    fn new() -> Tree {
        Tree { sys: sysinfo::System::new().ok() }
    }

    fn refresh(&mut self) {
        if let Some(s) = self.sys.as_mut() {
            s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        }
    }

    #[allow(dead_code)]
    fn memory_of(&self, pid: u32) -> Option<u64> {
        self.sys.as_ref()?.process(sysinfo::Pid::from_u32(pid)).map(|p| p.memory())
    }

    #[allow(dead_code)]
    fn usage(&self, root: u32) -> (u64, f32, usize) {
        use sysinfo::Pid;
        let Some(sys) = self.sys.as_ref() else { return (0, 0.0, 0) };
        let procs = sys.processes();
        let mut children: BTreeMap<Pid, Vec<Pid>> = BTreeMap::new();
        for (pid, p) in procs {
            if let Some(parent) = p.parent() {
                children.entry(parent).or_default().push(*pid);
            }
        }
        let (mut mem, mut cpu, mut n) = (0u64, 0f32, 0usize);
        let mut stack = vec![Pid::from_u32(root)];
        while let Some(pid) = stack.pop() {
            if let Some(p) = procs.get(&pid) {
                mem += p.memory();
                cpu += p.cpu_usage();
                n += 1;
            }
            if let Some(c) = children.get(&pid) {
                stack.extend(c.iter().copied());
            }
        }
        (mem, cpu, n)
    }
}

#[cfg(unix)]
pub mod process {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    #[derive(Default)]
    struct State {
        oom: bool,
    }

    pub struct Process {
        tree: Tree,
        last_refresh: Option<std::time::Instant>,
        state: HashMap<String, State>,
    }

    impl Process {
        pub fn new() -> Process {
            Process { tree: Tree::new(), last_refresh: None, state: HashMap::new() }
        }
    }

    fn group_alive(pgid: u32) -> bool {
        let r = unsafe { libc::kill(-(pgid as i32), 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    fn signal_group(pgid: u32, sig: i32) {
        unsafe {
            libc::kill(-(pgid as i32), sig);
        }
    }

    impl Supervisor for Process {
        fn name(&self) -> &'static str {
            "process"
        }

        fn spawn(&mut self, s: &Spawn) -> Result<Handle, String> {
            // macOS: run the phase in the background QoS band, which throttles
            // CPU and I/O the way CPUWeight=idle does on Linux.
            let taskpolicy = Path::new("/usr/sbin/taskpolicy");
            let mut c = if s.idle && cfg!(target_os = "macos") && taskpolicy.exists() {
                let mut c = Command::new(taskpolicy);
                c.arg("-b").arg(&s.program);
                c
            } else {
                Command::new(&s.program)
            };
            c.args(&s.args).current_dir(s.dir).envs(s.env);
            c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let nice = if s.idle { s.nice } else { 0 };
            unsafe {
                c.pre_exec(move || {
                    libc::setsid();
                    if nice > 0 {
                        libc::setpriority(libc::PRIO_PROCESS, 0, nice);
                    }
                    Ok(())
                });
            }
            let mut child = c.spawn().map_err(|e| format!("could not start {}: {e}", s.program.display()))?;
            let pid = child.id();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            self.state.insert(s.name.clone(), State::default());
            Ok(Handle { name: s.name.clone(), pid: Some(pid), mem_cap: s.mem })
        }

        fn poll(&mut self, h: &Handle) -> Usage {
            let Some(pid) = h.pid else { return Usage::default() };
            if !group_alive(pid) {
                return Usage::default();
            }
            // One process table refresh per tick, shared by every run.
            if self.last_refresh.is_none_or(|t| t.elapsed().as_millis() > 500) {
                self.tree.refresh();
                self.last_refresh = Some(std::time::Instant::now());
            }
            let (mem, cpu, n) = self.tree.usage(pid);
            if let Some(cap) = h.mem_cap {
                if mem > cap {
                    // The soft cap: the kernel will not do it here, so we do.
                    self.state.entry(h.name.clone()).or_default().oom = true;
                    signal_group(pid, libc::SIGKILL);
                }
            }
            Usage { alive: true, mem: (n > 0).then_some(mem), cpu: (n > 0).then_some(cpu) }
        }

        fn stop(&mut self, h: &Handle) {
            let Some(pid) = h.pid else { return };
            signal_group(pid, libc::SIGTERM);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(10));
                if group_alive(pid) {
                    signal_group(pid, libc::SIGKILL);
                }
            });
        }

        fn post_mortem(&mut self, h: &Handle) -> Ended {
            if self.state.get(&h.name).is_some_and(|s| s.oom) {
                Ended::Oom
            } else {
                Ended::Unknown
            }
        }

        fn forget(&mut self, h: &Handle, _failed: bool) {
            self.state.remove(&h.name);
        }
    }
}

// ---------------------------------------------------------------- job

#[cfg(windows)]
pub mod job {
    use super::*;
    use std::collections::HashMap;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, IDLE_PRIORITY_CLASS};

    const JOB_OBJECT_QUERY: u32 = 0x0004;
    const JOB_OBJECT_TERMINATE: u32 = 0x0008;

    struct State {
        job: HANDLE,
        cpu_prev: Option<(i64, std::time::Instant)>,
    }

    pub struct Job {
        state: HashMap<String, State>,
        tree: Tree,
        last_refresh: Option<std::time::Instant>,
    }

    impl Job {
        pub fn new() -> Job {
            Job { state: HashMap::new(), tree: Tree::new(), last_refresh: None }
        }

        fn job_handle(&mut self, name: &str) -> Option<HANDLE> {
            if let Some(s) = self.state.get(name) {
                return Some(s.job);
            }
            // Adopted after a daemon restart: reopen the job by name.
            let wide = wide(&job_name(name));
            let h = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, wide.as_ptr()) };
            if h.is_null() {
                return None;
            }
            self.state.insert(name.to_string(), State { job: h, cpu_prev: None });
            Some(h)
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn job_name(name: &str) -> String {
        format!("Local\\{name}")
    }

    fn accounting(job: HANDLE) -> Option<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            QueryInformationJobObject(
                job,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(info)
    }

    fn pids(job: HANDLE) -> Vec<u32> {
        // Room for 256 processes is plenty for one task.
        #[repr(C)]
        struct List {
            assigned: u32,
            in_list: u32,
            ids: [usize; 256],
        }
        let mut l: List = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            QueryInformationJobObject(
                job,
                JobObjectBasicProcessIdList,
                &mut l as *mut _ as *mut _,
                std::mem::size_of::<List>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return vec![];
        }
        l.ids[..(l.in_list as usize).min(256)].iter().map(|p| *p as u32).collect()
    }

    impl Supervisor for Job {
        fn name(&self) -> &'static str {
            "job"
        }

        fn spawn(&mut self, s: &Spawn) -> Result<Handle, String> {
            let wide_name = wide(&job_name(&s.name));
            let job = unsafe { CreateJobObjectW(std::ptr::null(), wide_name.as_ptr()) };
            if job.is_null() {
                return Err(format!("could not create a job object: {}", std::io::Error::last_os_error()));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            if let Some(m) = s.mem {
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                limits.JobMemoryLimit = m as usize;
            }
            if s.idle {
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PRIORITY_CLASS;
                limits.BasicLimitInformation.PriorityClass = IDLE_PRIORITY_CLASS;
            }
            if limits.BasicLimitInformation.LimitFlags != 0 {
                unsafe {
                    SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        &limits as *const _ as *const _,
                        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                }
            }
            let mut flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
            if s.idle {
                flags |= IDLE_PRIORITY_CLASS;
            }
            let mut c = Command::new(&s.program);
            c.args(&s.args).current_dir(s.dir).envs(s.env).creation_flags(flags);
            c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let mut child = match c.spawn() {
                Ok(c) => c,
                Err(e) => {
                    unsafe { CloseHandle(job) };
                    return Err(format!("could not start {}: {e}", s.program.display()));
                }
            };
            unsafe {
                AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
            }
            let pid = child.id();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            self.state.insert(s.name.clone(), State { job, cpu_prev: None });
            Ok(Handle { name: s.name.clone(), pid: Some(pid), mem_cap: s.mem })
        }

        fn poll(&mut self, h: &Handle) -> Usage {
            let Some(job) = self.job_handle(&h.name) else { return Usage::default() };
            let Some(acct) = accounting(job) else { return Usage::default() };
            if acct.ActiveProcesses == 0 {
                return Usage::default();
            }
            let busy = acct.TotalUserTime + acct.TotalKernelTime; // 100 ns units
            let now = std::time::Instant::now();
            let st = self.state.get_mut(&h.name).unwrap();
            let mut cpu = None;
            if let Some((prev, at)) = st.cpu_prev {
                let wall = now.duration_since(at).as_nanos().max(1) as f64 / 100.0;
                cpu = Some(((busy - prev) as f64 / wall * 100.0) as f32);
            }
            st.cpu_prev = Some((busy, now));
            if self.last_refresh.is_none_or(|t| t.elapsed().as_millis() > 500) {
                self.tree.refresh();
                self.last_refresh = Some(now);
            }
            let mem: u64 = pids(job).into_iter().filter_map(|p| self.tree.memory_of(p)).sum();
            Usage { alive: true, mem: (mem > 0).then_some(mem), cpu }
        }

        fn stop(&mut self, h: &Handle) {
            if let Some(job) = self.job_handle(&h.name) {
                unsafe { TerminateJobObject(job, 1) };
            }
        }

        fn post_mortem(&mut self, h: &Handle) -> Ended {
            let Some(job) = self.job_handle(&h.name) else { return Ended::Unknown };
            let mut v: JOBOBJECT_LIMIT_VIOLATION_INFORMATION = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    job,
                    JobObjectLimitViolationInformation,
                    &mut v as *mut _ as *mut _,
                    std::mem::size_of::<JOBOBJECT_LIMIT_VIOLATION_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok != 0 && v.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0 {
                Ended::Oom
            } else {
                Ended::Unknown
            }
        }

        fn forget(&mut self, h: &Handle, _failed: bool) {
            if let Some(s) = self.state.remove(&h.name) {
                unsafe { CloseHandle(s.job) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs() {
        let sh = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(which(sh).is_some());
        assert!(which("definitely-not-a-program-xyz").is_none());
        assert!(!default_shell().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn process_backend_runs_and_stops() {
        let mut p = process::Process::new();
        let env = BTreeMap::new();
        let dir = std::env::temp_dir();
        let h = p
            .spawn(&Spawn {
                name: "yard-test".into(),
                description: "test".into(),
                dir: &dir,
                env: &env,
                program: "sh".into(),
                args: vec!["-c".into(), "sleep 30".into()],
                mem: None,
                idle: true,
                nice: 19,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(p.poll(&h).alive);
        p.stop(&h);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!p.poll(&h).alive);
        assert_eq!(p.post_mortem(&h), Ended::Unknown);
    }
}
