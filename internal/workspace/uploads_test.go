package workspace

import (
	"bytes"
	"encoding/json"
	"io"
	"mime/multipart"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func uploadStore(t *testing.T) *Store {
	t.Helper()
	s := New("/bin/sh", Sandbox{root: t.TempDir()})
	s.sessions = []*session{{ID: "session", panes: map[string]*pane{"block": {}}}}
	if err := s.EnableUploads(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(s.uploads.close)
	return s
}
func uploadBody(t *testing.T, names ...string) (*bytes.Buffer, string) {
	t.Helper()
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	for _, name := range names {
		part, err := writer.CreateFormFile("file", name)
		if err != nil {
			t.Fatal(err)
		}
		part.Write([]byte("hello upload"))
	}
	writer.Close()
	return &body, writer.FormDataContentType()
}
func sendUpload(s *Store, body io.Reader, mime string) *httptest.ResponseRecorder {
	request := httptest.NewRequest("POST", "/api/upload?session=session&block=block", body)
	request.Header.Set("Content-Type", mime)
	response := httptest.NewRecorder()
	s.upload(response, request)
	return response
}
func TestUploadGeneratedPathAndPermissions(t *testing.T) {
	s := uploadStore(t)
	body, mime := uploadBody(t, "../../outside.txt")
	response := sendUpload(s, body, mime)
	if response.Code != 200 {
		t.Fatal(response.Body.String())
	}
	var result struct {
		Name, Path string
		Size       int64
	}
	if err := json.Unmarshal(response.Body.Bytes(), &result); err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(result.Name, "upload-") || filepath.Dir(result.Path) != s.sandbox.Root() {
		t.Fatal(result)
	}
	data, err := os.ReadFile(result.Path)
	if err != nil || string(data) != "hello upload" {
		t.Fatal(string(data), err)
	}
	info, _ := os.Stat(result.Path)
	if info.Mode().Perm() != 0600 {
		t.Fatal(info.Mode())
	}
}
func TestUploadRejectsPartialAndMultipleFiles(t *testing.T) {
	s := uploadStore(t)
	body, mime := uploadBody(t, "one", "two")
	if response := sendUpload(s, body, mime); response.Code != 400 {
		t.Fatal(response.Code)
	}
	body, mime = uploadBody(t, "one")
	data := body.Bytes()
	data = data[:len(data)-20]
	if response := sendUpload(s, bytes.NewReader(data), mime); response.Code != 400 {
		t.Fatal(response.Code)
	}
	entries, err := os.ReadDir(s.sandbox.Root())
	if err != nil || len(entries) != 0 {
		t.Fatal(entries, err)
	}
}
func TestUploadPinnedDirectoryDoesNotFollowReplacementSymlink(t *testing.T) {
	s := uploadStore(t)
	original := s.sandbox.Root()
	moved := original + "-original"
	if err := os.Rename(original, moved); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Remove(original); os.RemoveAll(moved) })
	outside := t.TempDir()
	if err := os.Symlink(outside, original); err != nil {
		t.Fatal(err)
	}
	body, mime := uploadBody(t, "outside")
	response := sendUpload(s, body, mime)
	if response.Code != 200 {
		t.Fatal(response.Body.String())
	}
	var result struct{ Name string }
	json.Unmarshal(response.Body.Bytes(), &result)
	if _, err := os.Stat(filepath.Join(moved, result.Name)); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(outside, result.Name)); !os.IsNotExist(err) {
		t.Fatal("upload escaped to symlink target")
	}
	if _, err := openUploadRoot(original); err == nil {
		t.Fatal("accepted symlink root")
	}
}
func TestUploadLimitsAndDisabled(t *testing.T) {
	s := uploadStore(t)
	body, mime := uploadBody(t, "file")
	request := httptest.NewRequest("POST", "/api/upload?session=session&block=block", body)
	request.Header.Set("Content-Type", mime)
	request.ContentLength = MaxUploadBytes + 65537
	response := httptest.NewRecorder()
	s.upload(response, request)
	if response.Code != 413 {
		t.Fatal(response.Code)
	}
	for i := 0; i < cap(s.uploads.slots); i++ {
		s.uploads.slots <- struct{}{}
	}
	body, mime = uploadBody(t, "file")
	if response := sendUpload(s, body, mime); response.Code != 503 {
		t.Fatal(response.Code)
	}
	for len(s.uploads.slots) > 0 {
		<-s.uploads.slots
	}
	s.uploads.close()
	s.uploads = nil
	body, mime = uploadBody(t, "file")
	if response := sendUpload(s, body, mime); response.Code != 403 {
		t.Fatal(response.Code)
	}
}
