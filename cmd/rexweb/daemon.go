package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"time"
)

type daemonOptions struct {
	socket, shell, workdir, rexListen string
}

func prepareSocketDir(socket string) (string, error) {
	dir := filepath.Dir(socket)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return "", err
	}
	info, err := os.Lstat(dir)
	if err != nil {
		return "", err
	}
	if !info.IsDir() || info.Mode().Perm()&0077 != 0 {
		return "", fmt.Errorf("session socket directory must be private: %s", dir)
	}
	return dir, nil
}

func lockDaemon(dir string) (*os.File, error) {
	file, err := os.OpenFile(filepath.Join(dir, "daemon.lock"), os.O_CREATE|os.O_RDWR|syscall.O_NOFOLLOW, 0600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		file.Close()
		return nil, fmt.Errorf("a session daemon is already starting or running: %w", err)
	}
	return file, nil
}

// The lifetime lock prevents another daemon from replacing the socket during this check.
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

func daemonClient(socket string) *http.Client {
	return &http.Client{
		Timeout: 500 * time.Millisecond,
		Transport: &http.Transport{DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			return (&net.Dialer{}).DialContext(ctx, "unix", socket)
		}},
	}
}

func daemonReady(client *http.Client) error {
	response, err := client.Post("http://localhost/api/action", "application/json", bytes.NewBufferString(`{"op":"ensure-default"}`))
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return fmt.Errorf("session daemon returned HTTP %d", response.StatusCode)
	}
	var session struct {
		ID string `json:"session_id"`
	}
	if err := json.NewDecoder(io.LimitReader(response.Body, 64<<10)).Decode(&session); err != nil {
		return err
	}
	if session.ID == "" {
		return errors.New("session daemon returned no default session")
	}
	return nil
}

func ensureDaemon(options daemonOptions) error {
	client := daemonClient(options.socket)
	defer client.CloseIdleConnections()
	if err := daemonReady(client); err == nil {
		return nil
	}

	dir, err := prepareSocketDir(options.socket)
	if err != nil {
		return err
	}
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	logPath := filepath.Join(dir, "daemon.log")
	output, err := os.OpenFile(logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY|syscall.O_NOFOLLOW, 0600)
	if err != nil {
		return err
	}
	args := []string{"-serve-sessions", "-session-socket", options.socket, "-shell", options.shell}
	if options.workdir != "" {
		args = append(args, "-workspace", options.workdir)
	}
	if options.rexListen != "" {
		args = append(args, "-rex-listen", options.rexListen)
	}
	cmd := exec.Command(executable, args...)
	cmd.Stdout, cmd.Stderr = output, output
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	err = cmd.Start()
	output.Close()
	if err != nil {
		return fmt.Errorf("start session daemon: %w", err)
	}
	exited := make(chan error, 1)
	go func() { exited <- cmd.Wait() }()
	log.Printf("Starting sandboxed session daemon, log %s", logPath)
	deadline := time.NewTimer(8 * time.Second)
	defer deadline.Stop()
	tick := time.NewTicker(100 * time.Millisecond)
	defer tick.Stop()
	var startupErr error
	for {
		if err := daemonReady(client); err == nil {
			return nil
		}
		select {
		case err := <-exited:
			startupErr = err
		case <-deadline.C:
			return fmt.Errorf("session daemon did not become ready (%v), inspect %s", startupErr, logPath)
		case <-tick.C:
		}
	}
}
