package workspace

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"unicode"

	"golang.org/x/sys/unix"
)

const MaxUploadBytes = 16 << 20

type uploadRoot struct {
	mu    sync.Mutex
	fd    int
	path  string
	slots chan struct{}
}

func openUploadRoot(path string) (*uploadRoot, error) {
	fd, err := unix.Open(path, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0)
	if err != nil {
		return nil, err
	}
	return &uploadRoot{fd: fd, path: path, slots: make(chan struct{}, 4)}, nil
}
func (u *uploadRoot) close() {
	u.mu.Lock()
	defer u.mu.Unlock()
	if u.fd >= 0 {
		unix.Close(u.fd)
		u.fd = -1
	}
}
func (u *uploadRoot) duplicate() (int, error) {
	u.mu.Lock()
	defer u.mu.Unlock()
	if u.fd < 0 {
		return 0, errors.New("uploads closed")
	}
	return unix.FcntlInt(uintptr(u.fd), unix.F_DUPFD_CLOEXEC, 3)
}
func (s *Store) EnableUploads() error {
	root, err := openUploadRoot(s.sandbox.Root())
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.uploads != nil {
		root.close()
		return errors.New("uploads already configured")
	}
	s.uploads = root
	return nil
}
func (s *Store) capabilities(w http.ResponseWriter, _ *http.Request) {
	s.mu.Lock()
	enabled := s.uploads != nil
	s.mu.Unlock()
	respond(w, fields{"uploads": enabled, "max_upload_bytes": MaxUploadBytes}, nil)
}
func extension(mime string) string {
	switch mime {
	case "image/png":
		return ".png"
	case "image/jpeg":
		return ".jpg"
	case "image/gif":
		return ".gif"
	case "image/webp":
		return ".webp"
	case "application/pdf":
		return ".pdf"
	}
	if strings.HasPrefix(mime, "text/plain") {
		return ".txt"
	}
	return ".bin"
}
func safeDisplayName(name string) string {
	runes := []rune(strings.Map(func(r rune) rune {
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, filepath.Base(name)))
	if len(runes) > 128 {
		runes = runes[:128]
	}
	return string(runes)
}

func (s *Store) upload(w http.ResponseWriter, r *http.Request) {
	s.mu.Lock()
	root := s.uploads
	s.mu.Unlock()
	if root == nil {
		respond(w, nil, &apiError{403, "uploads are disabled"})
		return
	}
	if s.lookupPane(r.URL.Query().Get("session"), r.URL.Query().Get("block")) == nil {
		respond(w, nil, missing("pane"))
		return
	}
	select {
	case root.slots <- struct{}{}:
		defer func() { <-root.slots }()
	default:
		respond(w, nil, &apiError{503, "upload concurrency limit reached"})
		return
	}
	if r.ContentLength > MaxUploadBytes+(64<<10) {
		respond(w, nil, &apiError{413, "upload exceeds 16 MiB"})
		return
	}
	control := http.NewResponseController(w)
	control.SetReadDeadline(time.Now().Add(2 * time.Minute))
	control.SetWriteDeadline(time.Now().Add(2*time.Minute + 10*time.Second))
	r.Body = http.MaxBytesReader(w, r.Body, MaxUploadBytes+(64<<10))
	reader, err := r.MultipartReader()
	if err != nil {
		respond(w, nil, invalid("multipart upload"))
		return
	}
	part, err := reader.NextPart()
	if err != nil || part.FormName() != "file" || part.FileName() == "" {
		respond(w, nil, invalid("single file upload"))
		return
	}
	defer part.Close()
	prefix := make([]byte, 512)
	n, err := io.ReadFull(part, prefix)
	if err != nil && err != io.EOF && err != io.ErrUnexpectedEOF {
		respond(w, nil, invalid("file content"))
		return
	}
	prefix = prefix[:n]
	mime := http.DetectContentType(prefix)
	id := uuid()
	temporary := ".upload-" + id + ".part"
	name := "upload-" + id + extension(mime)
	directory, err := root.duplicate()
	if err != nil {
		respond(w, nil, &apiError{503, "uploads closed"})
		return
	}
	defer unix.Close(directory)
	descriptor, err := unix.Openat(directory, temporary, unix.O_CREAT|unix.O_EXCL|unix.O_WRONLY|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0600)
	if err != nil {
		respond(w, nil, err)
		return
	}
	file := os.NewFile(uintptr(descriptor), temporary)
	complete := false
	defer func() {
		file.Close()
		if !complete {
			unix.Unlinkat(directory, temporary, 0)
		}
	}()
	length, err := io.Copy(file, io.MultiReader(bytes.NewReader(prefix), io.LimitReader(part, MaxUploadBytes-int64(n)+1)))
	if length > MaxUploadBytes {
		respond(w, nil, &apiError{413, "upload exceeds 16 MiB"})
		return
	}
	if err != nil {
		respond(w, nil, &apiError{400, "upload interrupted"})
		return
	}
	if _, err = reader.NextPart(); err != io.EOF {
		respond(w, nil, invalid("exactly one file"))
		return
	}
	if err = r.Context().Err(); err != nil {
		respond(w, nil, &apiError{400, "upload interrupted"})
		return
	}
	if err = file.Sync(); err != nil {
		respond(w, nil, err)
		return
	}
	if err = file.Close(); err != nil {
		respond(w, nil, err)
		return
	}
	if err = unix.Renameat2(directory, temporary, directory, name, unix.RENAME_NOREPLACE); err != nil {
		respond(w, nil, fmt.Errorf("commit upload: %w", err))
		return
	}
	complete = true
	respond(w, fields{"name": name, "display_name": safeDisplayName(part.FileName()), "path": filepath.Join(root.path, name), "mime": mime, "size": length}, nil)
}
