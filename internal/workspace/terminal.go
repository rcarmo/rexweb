package workspace

import (
	"errors"
	"fmt"
	"os"
	"sync"
	"syscall"
	"time"

	"github.com/creack/pty"
)

const historyBytes = 512 << 10

type pane struct {
	id      string
	file    *os.File
	process *os.Process
	owner   string // Protected by Store.mu.
	inputMu sync.Mutex

	mu       sync.Mutex
	history  []byte
	end      int64
	updates  chan paneUpdate
	consumer string
	vt       terminalModes
	exitCode int
	size     pty.Winsize
	lagging  bool
	finished bool
}

type attachment struct {
	Offset int64 `json:"offset"`
	Reset  bool  `json:"reset"`
	replay []byte
	live   chan paneUpdate
	size   pty.Winsize
	exited bool
}

type paneUpdate struct {
	output []byte
	size   *pty.Winsize
}

func startPane(sandbox Sandbox, shell string) (*pane, error) {
	master, slave, err := pty.Open()
	if err != nil {
		return nil, err
	}
	defer slave.Close()
	if err := pty.Setsize(master, &pty.Winsize{Cols: 80, Rows: 24}); err != nil {
		master.Close()
		return nil, err
	}
	cmd := sandbox.command(slave.Name(), shell, "-i")
	cmd.Stdin, cmd.Stdout, cmd.Stderr = slave, slave, slave
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true, Setctty: true}
	if err := cmd.Start(); err != nil {
		master.Close()
		return nil, err
	}

	file, err := pollableMaster(master)
	master.Close()
	if err != nil {
		cmd.Process.Kill()
		cmd.Wait()
		return nil, err
	}

	p := &pane{id: newID("block"), file: file, process: cmd.Process, history: make([]byte, historyBytes), size: pty.Winsize{Cols: 80, Rows: 24}}
	wait := make(chan int, 1)
	go func() {
		cmd.Wait()
		code := 1
		if cmd.ProcessState != nil {
			code = cmd.ProcessState.ExitCode()
		}
		wait <- code
	}()
	go func() {
		buf := make([]byte, 16<<10)
		for {
			n, err := p.file.Read(buf)
			if n > 0 {
				p.publish(buf[:n])
			}
			if err != nil {
				break
			}
		}
		p.mu.Lock()
		p.file.Close()
		p.mu.Unlock()
		code := <-wait
		p.mu.Lock()
		p.exitCode = code
		p.finished = true
		if p.updates != nil && !p.lagging {
			close(p.updates)
		}
		p.mu.Unlock()
	}()
	return p, nil
}

// Reopen the descriptor with Go's poller so input deadlines and Close work on the PTY.
func pollableMaster(master *os.File) (*os.File, error) {
	raw, err := master.SyscallConn()
	if err != nil {
		return nil, err
	}
	var fd int
	var dupErr error
	if err := raw.Control(func(original uintptr) {
		syscall.ForkLock.RLock()
		defer syscall.ForkLock.RUnlock()
		fd, dupErr = syscall.Dup(int(original))
		if dupErr == nil {
			syscall.CloseOnExec(fd)
		}
	}); err != nil {
		return nil, err
	}
	if dupErr != nil {
		return nil, dupErr
	}
	if err := syscall.SetNonblock(fd, true); err != nil {
		syscall.Close(fd)
		return nil, err
	}
	file := os.NewFile(uintptr(fd), master.Name())
	if err := file.SetWriteDeadline(time.Time{}); err != nil {
		file.Close()
		return nil, fmt.Errorf("configure PTY polling: %w", err)
	}
	return file, nil
}

func (p *pane) publish(data []byte) {
	p.mu.Lock()
	defer p.mu.Unlock()
	index := int(p.end % historyBytes)
	n := copy(p.history[index:], data)
	copy(p.history, data[n:])
	p.end += int64(len(data))
	p.vt.update(data)
	if p.updates != nil && !p.lagging {
		p.sendUpdate(paneUpdate{output: append([]byte(nil), data...)})
	}
}

func (p *pane) sendUpdate(update paneUpdate) {
	if p.updates == nil || p.lagging {
		return
	}
	select {
	case p.updates <- update:
	default:
		close(p.updates)
		p.lagging = true
		// Keep ownership until the old connection closes.
	}
}

var errPaneAttached = errors.New("pane already has an active terminal connection")

func (p *pane) attach(offset int64) (attachment, error) {
	return p.attachFor(offset, "")
}

func (p *pane) attachFor(offset int64, consumer string) (attachment, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.updates != nil {
		return attachment{}, errPaneAttached
	}
	oldest := max(0, p.end-historyBytes)
	reset := offset < oldest || offset > p.end
	if reset {
		offset = oldest
	}
	replay := make([]byte, p.end-offset)
	n := copy(replay, p.history[int(offset%historyBytes):])
	copy(replay[n:], p.history)
	a := attachment{Offset: offset, Reset: reset, replay: replay, exited: p.finished, size: p.size}
	if !p.finished {
		a.live = make(chan paneUpdate, 32)
		p.updates, p.consumer = a.live, consumer
	}
	return a, nil
}

func (p *pane) detach(a attachment) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.updates == a.live {
		p.updates, p.consumer = nil, ""
		p.lagging = false
	}
}

func (p *pane) ended() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.finished && !p.lagging
}

func (p *pane) write(input string) error {
	p.inputMu.Lock()
	defer p.inputMu.Unlock()
	if err := p.file.SetWriteDeadline(time.Now().Add(2 * time.Second)); err != nil {
		return err
	}
	_, err := p.file.WriteString(input)
	return err
}

func (p *pane) resize(cols, rows int) error {
	return p.resizeCells(cols, rows, 0, 0)
}

func (p *pane) resizeCells(cols, rows, width, height int) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if width == 0 && height == 0 {
		width, height = int(p.size.X)/int(p.size.Cols), int(p.size.Y)/int(p.size.Rows)
	}
	size := pty.Winsize{Cols: uint16(cols), Rows: uint16(rows), X: uint16(min(cols*width, 65535)), Y: uint16(min(rows*height, 65535))}
	if p.size == size {
		return nil
	}
	if err := pty.Setsize(p.file, &size); err != nil {
		return err
	}
	p.size = size
	p.sendUpdate(paneUpdate{size: &size})
	return nil
}

func (p *pane) currentSize() pty.Winsize {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.size
}

func (p *pane) canControl(id string) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.updates == nil || p.consumer == id
}

func (p *pane) close() {
	p.mu.Lock()
	p.file.Close()
	p.mu.Unlock()
	p.process.Kill()
}

func (p *pane) exitStatus() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.exitCode
}
