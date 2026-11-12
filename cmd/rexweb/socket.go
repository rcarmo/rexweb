package main

import (
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

func prepareSocketDir(socket string) (string, error) {
	dir := filepath.Dir(socket)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return "", err
	}
	info, err := os.Lstat(dir)
	if err != nil {
		return "", err
	}
	if stat, ok := info.Sys().(*syscall.Stat_t); !ok || stat.Uid != uint32(os.Getuid()) {
		return "", fmt.Errorf("socket directory must be owned by this account: %s", dir)
	}
	if !info.IsDir() || info.Mode().Perm()&0077 != 0 {
		return "", fmt.Errorf("session socket directory must be private: %s", dir)
	}
	return dir, nil
}

func lockInstance(dir string) (*os.File, error) {
	file, err := os.OpenFile(filepath.Join(dir, "service.lock"), os.O_CREATE|os.O_RDWR|syscall.O_NOFOLLOW, 0600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		file.Close()
		return nil, fmt.Errorf("a service instance is already starting or running: %w", err)
	}
	return file, nil
}

// The lifetime lock prevents another instance from replacing the socket during this check.
func removeStaleSocket(socket string) error {
	info, err := os.Lstat(socket)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSocket == 0 {
		return fmt.Errorf("refusing to replace a non-socket file at %s", socket)
	}
	conn, err := net.DialTimeout("unix", socket, 500*time.Millisecond)
	if err == nil {
		conn.Close()
		return fmt.Errorf("a server is already listening on %s", socket)
	}
	if !errors.Is(err, syscall.ECONNREFUSED) && !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("check existing session socket: %w", err)
	}
	return os.Remove(socket)
}
