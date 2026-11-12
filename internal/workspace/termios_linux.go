package workspace

import (
	"syscall"
	"unsafe"
)

func (p *pane) inputMode() (fields, error) {
	raw, err := p.file.SyscallConn()
	if err != nil {
		return nil, err
	}
	var attrs syscall.Termios
	var ioctlErr syscall.Errno
	if err := raw.Control(func(fd uintptr) {
		_, _, ioctlErr = syscall.Syscall(syscall.SYS_IOCTL, fd, syscall.TCGETS, uintptr(unsafe.Pointer(&attrs)))
	}); err != nil {
		return nil, err
	}
	if ioctlErr != 0 {
		return nil, ioctlErr
	}
	return fields{
		"echo": attrs.Lflag&syscall.ECHO != 0, "canonical": attrs.Lflag&syscall.ICANON != 0,
		"signals": attrs.Lflag&syscall.ISIG != 0,
	}, nil
}
