package workspace

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"syscall"
	"unsafe"
)

// The same executable re-enters in fresh namespaces before any shell is started.
// No privileged daemon, setuid program, or external sandbox executable is needed.
func (s Sandbox) linuxCommand(tty, program string, args ...string) *exec.Cmd {
	executable, _ := os.Executable()
	cmd := exec.Command(executable, append([]string{"__sandbox", s.root, tty, s.stage, program}, args...)...)
	cmd.SysProcAttr = &syscall.SysProcAttr{
		Cloneflags:                 syscall.CLONE_NEWUSER | syscall.CLONE_NEWNS | syscall.CLONE_NEWNET | syscall.CLONE_NEWPID | syscall.CLONE_NEWIPC | syscall.CLONE_NEWUTS,
		UidMappings:                []syscall.SysProcIDMap{{ContainerID: 0, HostID: os.Getuid(), Size: 1}},
		GidMappings:                []syscall.SysProcIDMap{{ContainerID: 0, HostID: os.Getgid(), Size: 1}},
		GidMappingsEnableSetgroups: false,
		Pdeathsig:                  syscall.SIGKILL,
	}
	return cmd
}

// RunSandbox must only be invoked by the executable's internal helper dispatch.
// It changes only this helper's namespaces and then replaces it with the shell.
func RunSandbox(args []string) error {
	runtime.LockOSThread()
	if len(args) < 4 {
		return fmt.Errorf("invalid sandbox invocation")
	}
	root, tty, stage, program := args[0], args[1], args[2], args[3]
	if tty != "/dev/null" {
		syscall.CloseOnExec(3)
	}
	if !filepath.IsAbs(root) || !filepath.IsAbs(program) {
		return fmt.Errorf("absolute sandbox paths required")
	}
	if err := syscall.Mount("", "/", "", syscall.MS_REC|syscall.MS_PRIVATE, ""); err != nil {
		return fmt.Errorf("mount propagation: %w", err)
	}
	if err := syscall.Mount("tmpfs", stage, "tmpfs", syscall.MS_NOSUID|syscall.MS_NODEV, "mode=0700,size=16m"); err != nil {
		return fmt.Errorf("mount root: %w", err)
	}
	bind := func(source, target string, readonly bool) error {
		info, err := os.Stat(source)
		if os.IsNotExist(err) {
			return nil
		}
		if err != nil {
			return err
		}
		dest := filepath.Join(stage, target)
		if info.IsDir() {
			err = os.MkdirAll(dest, 0755)
		} else {
			err = os.MkdirAll(filepath.Dir(dest), 0755)
			if err == nil {
				var f *os.File
				f, err = os.OpenFile(dest, os.O_CREATE|os.O_WRONLY, 0600)
				if f != nil {
					f.Close()
				}
			}
		}
		if err != nil {
			return err
		}
		if err := syscall.Mount(source, dest, "", syscall.MS_BIND, ""); err != nil {
			return err
		}
		var mounted syscall.Statfs_t
		if err := syscall.Statfs(dest, &mounted); err != nil {
			return err
		}
		// User namespaces cannot clear locked attributes inherited from host mounts.
		preserve := uintptr(mounted.Flags) & (syscall.MS_RDONLY | syscall.MS_NOSUID | syscall.MS_NODEV | syscall.MS_NOEXEC | syscall.MS_NOATIME | syscall.MS_NODIRATIME | syscall.MS_RELATIME)
		flags := uintptr(syscall.MS_BIND|syscall.MS_REMOUNT|syscall.MS_NOSUID) | preserve
		if readonly {
			flags |= syscall.MS_RDONLY
		}
		return syscall.Mount("", dest, "", flags, "")
	}
	// Deliberately do not bind /home, /run, /tmp, the host /proc, or all of /etc.
	for _, path := range []string{"/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc/alternatives", "/etc/ld.so.cache", "/etc/localtime"} {
		if err := bind(path, path, true); err != nil {
			return fmt.Errorf("bind %s: %w", path, err)
		}
	}
	for _, path := range []string{"/dev/null", "/dev/zero", "/dev/random", "/dev/urandom", "/dev/tty", tty} {
		if err := bind(path, path, false); err != nil {
			return fmt.Errorf("bind device %s: %w", path, err)
		}
	}
	if err := bind(root, root, false); err != nil {
		return fmt.Errorf("bind workspace: %w", err)
	}
	if err := os.MkdirAll(filepath.Join(stage, "proc"), 0555); err != nil {
		return err
	}
	if err := syscall.Mount("proc", filepath.Join(stage, "proc"), "proc", syscall.MS_NOSUID|syscall.MS_NODEV|syscall.MS_NOEXEC, ""); err != nil {
		return fmt.Errorf("mount proc: %w", err)
	}
	if err := syscall.Mount("", stage, "", syscall.MS_REMOUNT|syscall.MS_RDONLY|syscall.MS_NOSUID|syscall.MS_NODEV, ""); err != nil {
		return fmt.Errorf("seal sandbox root: %w", err)
	}
	if err := syscall.Chroot(stage); err != nil {
		return fmt.Errorf("chroot: %w", err)
	}
	if err := os.Chdir(root); err != nil {
		return err
	}
	// Prevent setuid binaries and file capabilities from acquiring privileges.
	if _, _, e := syscall.Syscall6(syscall.SYS_PRCTL, 38, 1, 0, 0, 0, 0); e != 0 {
		return e
	}
	// Drop both the bounding set and effective/permitted capabilities so the
	// shell cannot remount read-only paths or change namespace/chroot boundaries.
	for cap := uintptr(0); cap < 64; cap++ {
		// Unsupported capability numbers return EINVAL. Other errors must
		// not leave a shell with a partially intact bounding set.
		if _, _, e := syscall.Syscall6(syscall.SYS_PRCTL, 24, cap, 0, 0, 0, 0); e != 0 && e != syscall.EINVAL {
			return fmt.Errorf("drop capability %d: %w", cap, e)
		}
	}
	header := struct {
		Version uint32
		PID     int32
	}{Version: 0x20080522}
	data := [2]struct{ Effective, Permitted, Inheritable uint32 }{}
	if _, _, e := syscall.RawSyscall(syscall.SYS_CAPSET, uintptr(unsafe.Pointer(&header)), uintptr(unsafe.Pointer(&data)), 0); e != 0 {
		return e
	}
	if err := syscall.Exec(program, args[3:], os.Environ()); err != nil {
		return fmt.Errorf("exec shell: %w", err)
	}
	return nil
}
