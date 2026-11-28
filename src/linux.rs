//! Linux process isolation and PTY support for Rex sessions.
//!
//! The helper re-executes this binary into fresh namespaces. There is no host
//! fallback when any isolation step fails.

use anyhow::{Context, Result, anyhow, bail, ensure};
use std::ffi::{CStr, CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const READY_FD: RawFd = 3;
const READY_LIMIT: usize = 4096;
const READY_TIMEOUT_MS: i32 = 5000;
// musl omits this Linux statvfs constant from libc's public exports.
const STAT_RELATIME: libc::c_ulong = 0x1000;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A prepared, private workspace and the explicit mount staging directory.
pub struct Sandbox {
    root: PathBuf,
    stage: PathBuf,
    host: bool,
}

impl Sandbox {
    /// Validate paths and permissions, and probe the sandbox fail-closed.
    pub fn prepare(root: &Path, socket_dir: &Path, host: bool) -> Result<Self> {
        if unsafe { libc::geteuid() } == 0 {
            bail!("run the service as an unprivileged user");
        }

        let home = account_home()?;
        let root = if root.as_os_str().is_empty() {
            private_temp_dir("rexweb-workspace-")?
        } else {
            if !root.is_absolute() {
                bail!("specify an absolute -workspace path");
            }
            root.to_path_buf()
        };
        let root = resolve_existing_path(&root).context("resolve workspace")?;
        require_separate(
            &root,
            &home,
            "the workspace must be outside the account home",
        )?;

        let run_dir = fs::canonicalize(socket_dir)
            .with_context(|| format!("resolve socket directory {}", socket_dir.display()))?;
        require_separate(
            &root,
            &run_dir,
            "the workspace and session socket directory must be separate",
        )?;

        for dir in [&root, &root.join(".home"), &root.join(".tmp")] {
            private_directory(dir)?;
        }

        let probe = create_temporary_file(&run_dir, "sandbox-check-")?;
        let probe_path = probe.path.clone();
        let _remove_probe = RemoveOnDrop(probe_path.clone());
        drop(probe.file);
        let stage = run_dir.join("sandbox-root");
        private_directory(&stage)?;

        let sandbox = Self { root, stage, host };
        if !host {
            let probe_arg = probe_path
                .to_str()
                .ok_or_else(|| anyhow!("non-UTF-8 probe path"))?;
            let (child, _ready) = sandbox.launch_program(
                Path::new("/dev/null"),
                "/bin/sh",
                &[
                    "-c",
                    "test -w \"$HOME\" && ! /bin/cat \"$1\" >/dev/null 2>&1",
                    "sandbox-check",
                    probe_arg,
                ],
                Stdio::null(),
                Stdio::piped(),
                Stdio::piped(),
            )?;
            let output = child.wait_with_output().context("wait for sandbox probe")?;
            if !output.status.success() {
                bail!(
                    "verify shell sandbox: {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
        }
        Ok(sandbox)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Open an 80x24 PTY and start the requested interactive shell.
    pub fn spawn(&self, shell: &str) -> Result<(File, Child)> {
        let shell_meta = fs::metadata(shell).context("start sandboxed shell")?;
        ensure!(
            shell_meta.is_file() && shell_meta.permissions().mode() & 0o111 != 0,
            "start sandboxed shell"
        );
        let (master, slave) = open_pty()?;
        resize(&master, 80, 24, 0, 0)?;
        let tty = pty_name(&slave)?;
        let stdout = Stdio::from(slave.try_clone().context("clone PTY for stdout")?);
        let stderr = Stdio::from(slave.try_clone().context("clone PTY for stderr")?);
        let (child, _ready) =
            self.launch_program(&tty, shell, &["-i"], Stdio::from(slave), stdout, stderr)?;
        Ok((master, child))
    }

    fn launch_program(
        &self,
        tty: &Path,
        program: &str,
        args: &[&str],
        stdin: Stdio,
        stdout: Stdio,
        stderr: Stdio,
    ) -> Result<(Child, ReadyReader)> {
        let mut command = if self.host {
            let mut command = Command::new(program);
            command.args(args);
            command
        } else {
            let executable = std::env::current_exe().context("locate sandbox executable")?;
            let mut command = Command::new(executable);
            command.arg("sandbox-exec");
            command
                .arg(&self.root)
                .arg(tty)
                .arg(&self.stage)
                .arg(program);
            command.args(args);
            command
        };
        command.current_dir(&self.root);
        command.env_clear();
        command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        command.env("HOME", self.root.join(".home"));
        command.env("ZDOTDIR", self.root.join(".home"));
        command.env("TMPDIR", self.root.join(".tmp"));
        command.env("PWD", &self.root);
        command.env("SHELL", program);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("LANG", "en_US.UTF-8");
        match Path::new(program).file_name().and_then(OsStr::to_str) {
            Some("sh" | "dash") => {
                command.env("PS1", "\u{1b}]133;A\u{7}$ \u{1b}]133;B\u{7}");
            }
            Some("bash") => {
                command.env("PS1", "\\[\u{1b}]133;A\u{7}\\]$ \\[\u{1b}]133;B\u{7}\\]");
            }
            Some("zsh") => {
                command.env("PS1", "%{\u{1b}]133;A\u{7}%}$ %{\u{1b}]133;B\u{7}%}");
            }
            _ => {}
        }

        command.stdin(stdin).stdout(stdout).stderr(stderr);
        let has_tty = tty != Path::new("/dev/null");
        let mut ready = if self.host {
            command_before_exec(&mut command, None, has_tty, false);
            ReadyReader::Host
        } else {
            let (reader, writer) = status_pipe()?;
            command_before_exec(&mut command, Some(writer.as_raw_fd()), has_tty, true);
            ReadyReader::Sandbox {
                reader,
                writer: Some(writer),
            }
        };
        let mut child = command.spawn().map_err(|error| {
            anyhow!(
                "start {} shell: spawn helper: {error:#}",
                if self.host { "host" } else { "sandboxed" }
            )
        })?;
        ready.parent_spawned();
        if let Err(error) = ready.complete() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "start {} shell: {error:#}",
                if self.host { "host" } else { "sandboxed" }
            ));
        }
        Ok((child, ready))
    }
}

/// Run the private `sandbox-exec` helper. The service's argv dispatcher must
/// call this when `argv[1]` is `sandbox-exec` and pass `argv[2..]`.
pub fn run_helper(args: &[String]) -> Result<()> {
    let result = run_helper_inner(args);
    if let Err(error) = &result {
        report_ready_error(error);
    }
    result
}

fn run_helper_inner(args: &[String]) -> Result<()> {
    if args.len() < 4 {
        bail!("invalid sandbox invocation");
    }
    let root = Path::new(&args[0]);
    let tty = Path::new(&args[1]);
    let stage = Path::new(&args[2]);
    let program = &args[3];
    if !root.is_absolute() || !stage.is_absolute() || !Path::new(program).is_absolute() {
        bail!("absolute sandbox paths required");
    }

    // The command's pre_exec hook entered the non-PID namespaces and installed
    // the one-to-one UID/GID maps before this binary was execed. NEWPID is
    // deliberately deferred: entering it prevents the Tokio runtime from
    // cloning worker threads. Once this helper forks, its single-threaded
    // child can create NEWPID safely, then fork the namespace PID 1 shell.
    // SAFETY: after fork, the parent only waits and exits. The child enters PID
    // namespace setup before allocating or using Rust synchronization state.
    let pid = cvt(unsafe { libc::fork() }).context("fork namespace helper")?;
    if pid != 0 {
        clear_ready_fd();
        let mut status = 0;
        loop {
            let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
            if waited == pid {
                break;
            }
            if waited == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                unsafe { libc::_exit(1) };
            }
        }
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else {
            1
        };
        unsafe { libc::_exit(code) };
    }
    if unsafe { libc::unshare(libc::CLONE_NEWPID) } == -1 {
        report_static_ready_error(b"create PID namespace: unshare failed\n");
        unsafe { libc::_exit(1) };
    }
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } == -1 {
        report_static_ready_error(b"bind namespace helper lifetime to parent\n");
        unsafe { libc::_exit(1) };
    }

    // A tiny wrapper waits outside the PID namespace, while the first forked
    // process becomes namespace PID 1 and is ultimately replaced by the shell.
    // SAFETY: the child is now single-threaded and its synchronous continuation
    // performs only local setup before exec.
    let pid = cvt(unsafe { libc::fork() }).context("create PID namespace init")?;
    if pid != 0 {
        clear_ready_fd();
        let mut status = 0;
        loop {
            let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
            if waited == pid {
                break;
            }
            if waited == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                unsafe { libc::_exit(1) };
            }
        }
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else {
            1
        };
        unsafe { libc::_exit(code) };
    }
    // SAFETY: after a successful fork in this freshly execed helper, the child
    // installs a synchronous parent-death signal before continuing setup.
    cvt(unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) })
        .context("bind namespace init lifetime to helper")?;
    cvt(unsafe {
        libc::mount(
            c_string("")?.as_ptr(),
            c_string("/")?.as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    })
    .context("mount propagation")?;
    cvt(unsafe {
        libc::mount(
            c_string("tmpfs")?.as_ptr(),
            c_path(stage)?.as_ptr(),
            c_string("tmpfs")?.as_ptr(),
            libc::MS_NOSUID | libc::MS_NODEV,
            c_string("mode=0700,size=16m")?.as_ptr().cast(),
        )
    })
    .context("mount root")?;

    for path in [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/etc/alternatives",
        "/etc/ld.so.cache",
        "/etc/localtime",
    ] {
        bind(stage, Path::new(path), true).with_context(|| format!("bind {path}"))?;
    }
    for path in [
        Path::new("/dev/null"),
        Path::new("/dev/zero"),
        Path::new("/dev/random"),
        Path::new("/dev/urandom"),
        Path::new("/dev/tty"),
        tty,
    ] {
        bind(stage, path, false).with_context(|| format!("bind device {}", path.display()))?;
    }
    bind(stage, root, false).context("bind workspace")?;

    let proc = staged_path(stage, Path::new("/proc"));
    fs::create_dir_all(&proc)?;
    fs::set_permissions(&proc, fs::Permissions::from_mode(0o555))?;
    cvt(unsafe {
        libc::mount(
            c_string("proc")?.as_ptr(),
            c_path(&proc)?.as_ptr(),
            c_string("proc")?.as_ptr(),
            libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
            std::ptr::null(),
        )
    })
    .context("mount proc")?;
    cvt(unsafe {
        libc::mount(
            std::ptr::null(),
            c_path(stage)?.as_ptr(),
            std::ptr::null(),
            libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
            std::ptr::null(),
        )
    })
    .context("seal sandbox root")?;
    cvt(unsafe { libc::chroot(c_path(stage)?.as_ptr()) }).context("chroot")?;
    std::env::set_current_dir(root).context("enter workspace")?;

    cvt(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) })
        .context("set no-new-privileges")?;
    drop_capabilities()?;

    clear_ready_fd();
    let program_c = c_string(program)?;
    let arguments = std::iter::once(program.to_owned())
        .chain(args[4..].iter().cloned())
        .map(|arg| CString::new(arg).context("shell argument contains NUL"))
        .collect::<Result<Vec<_>>>()?;
    let mut argument_ptrs: Vec<*const libc::c_char> =
        arguments.iter().map(|arg| arg.as_ptr()).collect();
    argument_ptrs.push(std::ptr::null());
    let env = std::env::vars_os()
        .map(|(key, value)| {
            let mut item = key.as_bytes().to_vec();
            item.push(b'=');
            item.extend_from_slice(value.as_bytes());
            CString::new(item).map_err(Into::into)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut env_ptrs: Vec<*const libc::c_char> = env.iter().map(|item| item.as_ptr()).collect();
    env_ptrs.push(std::ptr::null());
    // SAFETY: both vectors are NUL-terminated and their CString allocations
    // remain alive while execve borrows them. execve never returns on success.
    unsafe {
        libc::execve(
            program_c.as_ptr(),
            argument_ptrs.as_ptr(),
            env_ptrs.as_ptr(),
        );
    }
    Err(std::io::Error::last_os_error()).context("exec shell")
}

/// Read the PTY local-mode flags used by the Rex `input_mode` action.
///
/// The tuple order intentionally matches the wire fields: echo, canonical,
/// signals.
pub fn input_mode(file: &File) -> Result<(bool, bool, bool)> {
    let mut attrs = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: termios is the exact structure expected by TCGETS and remains
    // valid for the duration of this non-retained ioctl call.
    cvt(unsafe { libc::ioctl(file.as_raw_fd(), libc::TCGETS, &mut attrs) })
        .context("read PTY input mode")?;
    Ok((
        attrs.c_lflag & libc::ECHO != 0,
        attrs.c_lflag & libc::ICANON != 0,
        attrs.c_lflag & libc::ISIG != 0,
    ))
}

/// Apply terminal dimensions with the Linux PTY ioctl ABI.
pub fn resize(file: &File, cols: u16, rows: u16, width: u16, height: u16) -> Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: width,
        ws_ypixel: height,
    };
    // SAFETY: `winsize` is the exact structure expected by TIOCSWINSZ and
    // remains valid for the duration of the non-retained ioctl call.
    cvt(unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &size) }).context("resize PTY")?;
    Ok(())
}

fn open_pty() -> Result<(File, File)> {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty writes two valid descriptors on success; there is no
    // caller-owned termios or winsize memory to retain.
    cvt(unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    })
    .context("open PTY")?;
    // SAFETY: ownership of the newly opened descriptors transfers exactly once.
    Ok(unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) })
}

fn pty_name(slave: &File) -> Result<PathBuf> {
    let mut buffer = vec![0 as libc::c_char; 128];
    // ttyname_r returns an errno directly rather than reporting -1 and setting
    // errno, so cvt cannot be used here.
    // SAFETY: the initialized buffer has the supplied capacity and the C
    // function does not retain it.
    let error = unsafe { libc::ttyname_r(slave.as_raw_fd(), buffer.as_mut_ptr(), buffer.len()) };
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error)).context("name PTY");
    }
    // SAFETY: a successful ttyname_r always NUL-terminates the buffer within
    // its documented capacity. The bytes are copied into owned storage.
    let name = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes();
    Ok(PathBuf::from(std::ffi::OsString::from_vec(name.to_vec())))
}

fn command_before_exec(
    command: &mut Command,
    ready_fd: Option<RawFd>,
    controlling_tty: bool,
    sandbox: bool,
) {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    // SAFETY: the closure only performs async-signal-safe syscalls after fork.
    unsafe {
        command.pre_exec(move || {
            if controlling_tty && libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if controlling_tty && libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if let Some(fd) = ready_fd {
                if fd != READY_FD && libc::dup2(fd, READY_FD) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                let flags = libc::fcntl(READY_FD, libc::F_GETFD);
                if flags == -1
                    || libc::fcntl(READY_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if sandbox {
                let flags = libc::CLONE_NEWUSER
                    | libc::CLONE_NEWNS
                    | libc::CLONE_NEWNET
                    | libc::CLONE_NEWIPC
                    | libc::CLONE_NEWUTS;
                if libc::unshare(flags) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                write_proc_file(c"/proc/self/setgroups", b"deny\n")?;
                write_id_map(c"/proc/self/uid_map", uid)?;
                write_id_map(c"/proc/self/gid_map", gid)?;
            }
            Ok(())
        });
    }
}

// These helpers run in Command::pre_exec. They intentionally avoid allocation,
// formatting machinery and std I/O, all of which are unsafe in a forked child
// of a potentially multithreaded parent. Every operation is async-signal-safe.
fn write_proc_file(path: &CStr, data: &[u8]) -> std::io::Result<()> {
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if fd == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let result = write_raw(fd, data);
    let close_result = unsafe { libc::close(fd) };
    result?;
    if close_result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn write_id_map(path: &CStr, identity: libc::uid_t) -> std::io::Result<()> {
    let mut data = [0_u8; 64];
    let mut length = 2;
    data[0] = b'0';
    data[1] = b' ';
    let mut digits = [0_u8; 32];
    let mut count = 0;
    let mut value = identity;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for index in (0..count).rev() {
        data[length] = digits[index];
        length += 1;
    }
    data[length..length + 3].copy_from_slice(b" 1\n");
    write_proc_file(path, &data[..length + 3])
}

fn write_raw(fd: RawFd, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        let written = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if written > 0 {
            data = &data[written as usize..];
        } else if written == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "short write while installing sandbox identity map",
            ));
        }
    }
    Ok(())
}

fn bind(stage: &Path, source: &Path, readonly: bool) -> Result<()> {
    let metadata = match fs::metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let destination = staged_path(stage, source);
    if metadata.is_dir() {
        fs::create_dir_all(&destination)?;
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&destination)?;
    }
    cvt(unsafe {
        libc::mount(
            c_path(source)?.as_ptr(),
            c_path(&destination)?.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        )
    })?;

    let mut stat = unsafe { std::mem::zeroed::<libc::statvfs>() };
    // SAFETY: statfs initializes the struct on success.
    cvt(unsafe { libc::statvfs(c_path(&destination)?.as_ptr(), &mut stat) })?;
    // statvfs reports ST_* flags, which mostly share values with mount flags
    // but are a distinct ABI (notably ST_RELATIME). Translate explicitly so
    // inherited locked attributes are carried into the remount on all Linux
    // architectures.
    let preserved = [
        (libc::ST_RDONLY, libc::MS_RDONLY),
        (libc::ST_NOSUID, libc::MS_NOSUID),
        (libc::ST_NODEV, libc::MS_NODEV),
        (libc::ST_NOEXEC, libc::MS_NOEXEC),
        (libc::ST_NOATIME, libc::MS_NOATIME),
        (libc::ST_NODIRATIME, libc::MS_NODIRATIME),
        (STAT_RELATIME, libc::MS_RELATIME),
    ]
    .into_iter()
    .fold(0, |flags, (stat_flag, mount_flag)| {
        flags
            | if stat.f_flag & stat_flag != 0 {
                mount_flag
            } else {
                0
            }
    });
    let mut flags = libc::MS_BIND | libc::MS_REMOUNT | libc::MS_NOSUID | preserved;
    if readonly {
        flags |= libc::MS_RDONLY;
    }
    cvt(unsafe {
        libc::mount(
            std::ptr::null(),
            c_path(&destination)?.as_ptr(),
            std::ptr::null(),
            flags,
            std::ptr::null(),
        )
    })
    .map(|_| ())
    .with_context(|| format!("remount {}", destination.display()))
}

fn drop_capabilities() -> Result<()> {
    for capability in 0..64 {
        // Linux reports EINVAL for capability numbers unsupported by this kernel.
        let result = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, capability, 0, 0, 0) };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINVAL) {
                return Err(error).with_context(|| format!("drop capability {capability}"));
            }
        }
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let header = Header {
        version: 0x2008_0522,
        pid: 0,
    };
    let data = [Data {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    // SAFETY: these are the documented Linux v3 capability header/data ABI;
    // pointers and storage remain valid for capset and are not retained.
    cvt(unsafe { libc::syscall(libc::SYS_capset, &header, data.as_ptr()) } as i32)
        .context("clear effective, permitted and inheritable capabilities")?;
    Ok(())
}

fn account_home() -> Result<PathBuf> {
    // SAFETY: getpwuid returns a process-lifetime libc passwd entry for the
    // current UID, and the copied bytes are immediately converted to owned data.
    let passwd = unsafe { libc::getpwuid(libc::getuid()) };
    if passwd.is_null() {
        return Err(std::io::Error::last_os_error()).context("find the account home");
    }
    // SAFETY: a non-null passwd from getpwuid contains a valid NUL-terminated
    // pw_dir; CStr copies it before returning from this function.
    let home = unsafe { CStr::from_ptr((*passwd).pw_dir) }.to_bytes();
    let path = PathBuf::from(std::ffi::OsString::from_vec(home.to_vec()));
    fs::canonicalize(path).context("resolve the account home")
}

fn resolve_existing_path(path: &Path) -> Result<PathBuf> {
    if path == Path::new("/") {
        return fs::canonicalize(path).map_err(Into::into);
    }
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(path) {
                Ok(_) => bail!("cannot resolve workspace path {}", path.display()),
                Err(link_error) if link_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(link_error) => return Err(link_error.into()),
            }
            let parent = path
                .parent()
                .ok_or_else(|| anyhow!("workspace has no parent"))?;
            let name = path
                .file_name()
                .ok_or_else(|| anyhow!("workspace has no basename"))?;
            Ok(resolve_existing_path(parent)?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

fn require_separate(first: &Path, second: &Path, message: &str) -> Result<()> {
    for (parent_path, child_path) in [(first, second), (second, first)] {
        let parent = match fs::metadata(parent_path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut path = child_path;
        loop {
            match fs::metadata(path) {
                Ok(child) if same_file(&parent, &child) => bail!("{message}"),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let Some(next) = path.parent() else { break };
            if next == path {
                break;
            }
            path = next;
        }
    }
    Ok(())
}

fn same_file(first: &fs::Metadata, second: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    first.dev() == second.dev() && first.ino() == second.ino()
}

fn private_directory(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let metadata = fs::symlink_metadata(dir)?;
    use std::os::unix::fs::MetadataExt;
    if !metadata.file_type().is_dir() || metadata.mode() & 0o077 != 0 {
        bail!("workspace directory must be private: {}", dir.display());
    }
    Ok(())
}

fn private_temp_dir(prefix: &str) -> Result<PathBuf> {
    for _ in 0..100 {
        let path = std::env::temp_dir().join(unique_name(prefix));
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("create temporary workspace")
}

struct TemporaryFile {
    path: PathBuf,
    file: File,
}

fn create_temporary_file(directory: &Path, prefix: &str) -> Result<TemporaryFile> {
    for _ in 0..100 {
        let path = directory.join(unique_name(prefix));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok(TemporaryFile { path, file }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("create sandbox probe")
}

fn unique_name(prefix: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}{}-{nonce}-{counter}", std::process::id())
}

struct RemoveOnDrop(PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn status_pipe() -> Result<(File, File)> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 initializes both integers on success and grants ownership
    // of exactly one read and one write descriptor.
    cvt(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) })
        .context("create sandbox status pipe")?;
    // SAFETY: each fresh descriptor is owned by exactly one File.
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}

enum ReadyReader {
    Host,
    Sandbox { reader: File, writer: Option<File> },
}
impl ReadyReader {
    fn parent_spawned(&mut self) {
        if let ReadyReader::Sandbox { writer, .. } = self {
            drop(writer.take());
        }
    }

    fn complete(&self) -> Result<()> {
        let ReadyReader::Sandbox { reader, .. } = self else {
            return Ok(());
        };
        let mut poll = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };
        // SAFETY: poll observes the single initialized pollfd synchronously.
        cvt(unsafe { libc::poll(&mut poll, 1, READY_TIMEOUT_MS) })?;
        if poll.revents == 0 {
            bail!("timed out waiting for sandbox setup");
        }
        let mut message = Vec::new();
        Read::by_ref(&mut &*reader)
            .take(READY_LIMIT as u64)
            .read_to_end(&mut message)?;
        if message.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(String::from_utf8_lossy(&message).trim().to_owned()))
        }
    }
}

fn report_ready_error(error: &anyhow::Error) {
    // SAFETY: fd 3 is supplied by the parent exclusively as the status side
    // channel. A write of an owned byte slice is bounded and synchronous.
    let message = format!("{error:#}\n");
    unsafe { libc::write(READY_FD, message.as_ptr().cast(), message.len()) };
    clear_ready_fd();
}

fn report_static_ready_error(message: &[u8]) {
    let _ = write_raw(READY_FD, message);
    clear_ready_fd();
}

fn clear_ready_fd() {
    // SAFETY: closing the conventional status descriptor is idempotent.
    unsafe { libc::close(READY_FD) };
}

fn staged_path(stage: &Path, path: &Path) -> PathBuf {
    stage.join(path.strip_prefix("/").unwrap_or(path))
}

fn c_path(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes()).context("path contains NUL")
}

fn c_string(value: &str) -> Result<CString> {
    CString::new(value).context("string contains NUL")
}

fn cvt(value: i32) -> std::io::Result<i32> {
    if value == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(value)
    }
}

// `openpty` and termios are only available on Unix. This module is Linux by
// contract, so keeping these imports local makes the generated file's boundary
// explicit without trying to emulate isolation on another operating system.
use std::os::unix::fs::PermissionsExt;

#[cfg(test)]
mod tests {
    use super::{input_mode, open_pty};

    #[test]
    fn pty_input_mode_reports_echo_canonical_and_signals() {
        let (master, _slave) = open_pty().unwrap();
        let (echo, canonical, signals) = input_mode(&master).unwrap();
        assert!(echo);
        assert!(canonical);
        assert!(signals);
    }
}
