package workspace

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"runtime"
	"strings"
)

const seatbelt = "/usr/bin/sandbox-exec"

const shellProfile = `
(version 1)
(deny default)
(allow syscall*)
(import "dyld-support.sb")
(allow process-fork)
(allow signal (target same-sandbox))
(allow sysctl-read)
(allow file-read-metadata)
(allow process-exec file-map-executable
  (subpath (param "WORKSPACE"))
  (subpath "/bin") (subpath "/sbin") (subpath "/usr/bin") (subpath "/usr/sbin"))
(allow file-map-executable (subpath "/System/Library") (subpath "/usr/lib"))
(allow file-read*
  (subpath (param "WORKSPACE"))
  (subpath "/System/Library") (subpath "/usr/lib") (subpath "/usr/share")
  (subpath "/bin") (subpath "/sbin") (subpath "/usr/bin") (subpath "/usr/sbin")
  (literal "/private/etc/localtime") (subpath "/private/var/db/timezone")
  (literal "/private/etc/zshenv") (literal "/private/etc/zshrc")
  (literal "/dev/null") (literal "/dev/zero")
  (literal "/dev/random") (literal "/dev/urandom")
  (literal "/dev/tty") (literal (param "TTY")))
(allow file-write* (subpath (param "WORKSPACE")))
(allow file-write-data file-ioctl
  (literal "/dev/null") (literal "/dev/zero")
  (literal "/dev/tty") (literal (param "TTY")))
(allow mach-lookup (global-name "com.apple.system.logger"))
`

// Sandbox holds the directory and environment shared by sandboxed shells.
type Sandbox struct{ root string }

func (s Sandbox) Root() string { return s.root }

// PrepareSandbox fails before the daemon listens if its shell policy cannot be applied.
func PrepareSandbox(root, socketDir string) (Sandbox, error) {
	if runtime.GOOS != "darwin" {
		return Sandbox{}, errors.New("sandboxed sessions currently require macOS and sandbox-exec")
	}
	if os.Geteuid() == 0 {
		return Sandbox{}, errors.New("run the session daemon as an unprivileged user")
	}
	account, err := user.Current()
	if err != nil {
		return Sandbox{}, fmt.Errorf("find the account home: %w", err)
	}
	home, err := filepath.EvalSymlinks(account.HomeDir)
	if err != nil {
		return Sandbox{}, fmt.Errorf("resolve the account home: %w", err)
	}
	if root == "" {
		root, err = os.MkdirTemp("/private/tmp", "rexweb-workspace-")
		if err != nil {
			return Sandbox{}, err
		}
	} else if !filepath.IsAbs(root) {
		return Sandbox{}, errors.New("specify an absolute -workspace path")
	}
	root = filepath.Clean(root)
	// Resolve existing parents before creating anything. This also catches a missing
	// directory reached through a symlink into the account home.
	root, err = resolveWorkspace(root)
	if err != nil {
		return Sandbox{}, err
	}
	if err := requireSeparate(root, home, "the workspace must be outside the account home"); err != nil {
		return Sandbox{}, err
	}
	runDir, err := filepath.EvalSymlinks(socketDir)
	if err != nil {
		return Sandbox{}, err
	}
	if err := requireSeparate(root, runDir, "the workspace and session socket directory must be separate"); err != nil {
		return Sandbox{}, err
	}
	for _, dir := range []string{root, filepath.Join(root, ".home"), filepath.Join(root, ".tmp")} {
		if err := privateDirectory(dir); err != nil {
			return Sandbox{}, err
		}
	}

	probe, err := os.CreateTemp(runDir, "sandbox-check-")
	if err != nil {
		return Sandbox{}, err
	}
	probe.Close()
	defer os.Remove(probe.Name())

	sandbox := Sandbox{root: root}
	cmd := sandbox.command("/dev/null", "/bin/sh", "-c",
		`test -w "$HOME" && ! /bin/cat "$1" >/dev/null 2>&1`, "sandbox-check", probe.Name())
	if output, err := cmd.CombinedOutput(); err != nil {
		return Sandbox{}, fmt.Errorf("verify macOS shell sandbox: %w: %s", err, strings.TrimSpace(string(output)))
	}
	return sandbox, nil
}

func resolveWorkspace(path string) (string, error) {
	resolved, err := filepath.EvalSymlinks(path)
	if err == nil || !errors.Is(err, os.ErrNotExist) || path == "/" {
		return resolved, err
	}
	if _, err := os.Lstat(path); !errors.Is(err, os.ErrNotExist) {
		return "", fmt.Errorf("cannot resolve workspace path %s", path)
	}
	parent, err := resolveWorkspace(filepath.Dir(path))
	if err != nil {
		return "", err
	}
	return filepath.Join(parent, filepath.Base(path)), nil
}

func requireSeparate(first, second, message string) error {
	for _, pair := range [][2]string{{first, second}, {second, first}} {
		parent, err := os.Stat(pair[0])
		if errors.Is(err, os.ErrNotExist) {
			continue
		}
		if err != nil {
			return err
		}
		for path := pair[1]; ; path = filepath.Dir(path) {
			child, err := os.Stat(path)
			if err == nil && os.SameFile(parent, child) {
				return errors.New(message)
			}
			if err != nil && !errors.Is(err, os.ErrNotExist) {
				return err
			}
			if filepath.Dir(path) == path {
				break
			}
		}
	}
	return nil
}

func privateDirectory(dir string) error {
	if err := os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	info, err := os.Lstat(dir)
	if err != nil {
		return err
	}
	if !info.IsDir() || info.Mode().Perm()&0077 != 0 {
		return fmt.Errorf("workspace directory must be private: %s", dir)
	}
	return nil
}

func (s Sandbox) command(tty, program string, args ...string) *exec.Cmd {
	argv := []string{"-p", shellProfile, "-D", "WORKSPACE=" + s.root, "-D", "TTY=" + tty, program}
	cmd := exec.Command(seatbelt, append(argv, args...)...)
	cmd.Dir = s.root
	home := filepath.Join(s.root, ".home")
	cmd.Env = []string{
		"PATH=/usr/bin:/bin:/usr/sbin:/sbin",
		"HOME=" + home,
		"ZDOTDIR=" + home,
		"TMPDIR=" + filepath.Join(s.root, ".tmp"),
		"PWD=" + s.root,
		"SHELL=" + program,
		"TERM=xterm-256color",
		"COLORTERM=truecolor",
		"LANG=en_US.UTF-8",
	}
	return cmd
}
