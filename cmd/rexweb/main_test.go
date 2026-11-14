package main

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"rexweb/internal/workspace"
	"strings"
	"testing"
	"testing/fstest"
)

func validOptions() options {
	return options{mode: "both", listen: "127.0.0.1:8080", rexListen: "127.0.0.1:7297", socket: "/tmp/private/session.sock", shell: "/bin/sh", isolation: "sandbox"}
}
func TestListenerPolicy(t *testing.T) {
	for _, test := range []struct {
		name   string
		change func(*options)
		valid  bool
	}{
		{"local", func(*options) {}, true},
		{"invalid mode", func(o *options) { o.mode = "wrong" }, false},
		{"invalid isolation", func(o *options) { o.isolation = "none" }, false},
		{"URL prefix", func(o *options) { o.basePath = "/terminal/" }, true},
		{"unsafe URL prefix", func(o *options) { o.basePath = "/../" }, false},
		{"unterminated URL prefix", func(o *options) { o.basePath = "/terminal" }, false},
		{"relative socket", func(o *options) { o.socket = "sock" }, false},
		{"remote web unguarded", func(o *options) { o.listen = ":8080" }, false},
		{"remote web HTTP", func(o *options) { o.listen = ":8080"; o.passwordFile = "/secret"; o.publicURL = "http://host:8080" }, false},
		{"remote web TLS", func(o *options) {
			o.listen = ":8080"
			o.passwordFile = "/secret"
			o.publicURL = "https://host:8080"
			o.cert = "cert"
			o.key = "key"
		}, true},
		{"remote native denied", func(o *options) { o.rexListen = ":7297" }, false},
		{"remote native explicit", func(o *options) { o.rexListen = ":7297"; o.anonymous = true }, true},
		{"invalid public URL", func(o *options) { o.publicURL = "https://host/path" }, false},
		{"TLS pair", func(o *options) { o.key = "key" }, false},
	} {
		t.Run(test.name, func(t *testing.T) {
			o := validOptions()
			test.change(&o)
			err := o.validate()
			if (err == nil) != test.valid {
				t.Fatalf("valid=%v err=%v", test.valid, err)
			}
		})
	}
}

func TestWebPrefix(t *testing.T) {
	store := workspace.New("/bin/sh", workspace.Sandbox{})
	defer store.Close()
	o := validOptions()
	o.basePath = "/terminal/"
	o.publicURL = "https://terminal.example"
	assets := fstest.MapFS{
		"index.html":             {Data: []byte(`<base href="__REXWEB_BASE_PATH__"><script src="static/js/workspace.js"></script>`)},
		"static/js/workspace.js": {Data: []byte("asset")},
	}
	h := webHandler(o, store, assets, "")
	for _, test := range []struct {
		path    string
		status  int
		content string
	}{
		{"/terminal/", 200, `<base href="/terminal/">`},
		{"/terminal/index.html", 200, `<base href="/terminal/">`},
		{"/terminal/static/js/workspace.js", 200, "asset"},
		{"/terminal/api/sessions", 200, `"sessions"`},
		{"/api/sessions", 404, ""},
		{"/terminal-extra/api/sessions", 404, ""},
	} {
		r := httptest.NewRequest("GET", test.path, nil)
		r.Host = "terminal.example"
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != test.status || !strings.Contains(w.Body.String(), test.content) {
			t.Errorf("%s: status=%d body=%s", test.path, w.Code, w.Body.String())
		}
	}
	r := httptest.NewRequest("POST", "/terminal/api/action", strings.NewReader(`{"op":"new-session"}`))
	r.Host = "terminal.example"
	r.Header.Set("Origin", "https://evil.example")
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatal("prefix bypassed Origin check")
	}
	protected := webHandler(o, store, assets, "synthetic-test-password")
	for _, test := range []struct {
		path, host, origin, password string
		status                       int
	}{
		{"/terminal/", "terminal.example", "", "", 401},
		{"/terminal/static/js/workspace.js", "terminal.example", "", "", 401},
		{"/terminal/api/terminal", "terminal.example", "https://terminal.example", "", 401},
		{"/terminal/api/sessions", "evil.example", "", "synthetic-test-password", 403},
		{"/terminal/api/terminal", "terminal.example", "https://evil.example", "synthetic-test-password", 403},
		{"/terminal/api/sessions", "terminal.example", "https://terminal.example", "synthetic-test-password", 200},
	} {
		r := httptest.NewRequest("GET", test.path, nil)
		r.Host = test.host
		r.Header.Set("Origin", test.origin)
		r.SetBasicAuth("rexweb", test.password)
		w := httptest.NewRecorder()
		protected.ServeHTTP(w, r)
		if w.Code != test.status {
			t.Errorf("prefixed boundary %s: got %d, want %d", test.path, w.Code, test.status)
		}
	}
	root := webHandler(validOptions(), store, assets, "")
	r = httptest.NewRequest("GET", "/", nil)
	r.Host = "localhost"
	w = httptest.NewRecorder()
	root.ServeHTTP(w, r)
	if w.Code != 200 || !strings.Contains(w.Body.String(), `<base href="/">`) {
		t.Fatal("default root prefix did not replace the HTML placeholder")
	}
}
func TestUploadRequiresWebAuthentication(t *testing.T) {
	o := validOptions()
	o.uploads = true
	if err := o.validate(); err == nil {
		t.Fatal("anonymous upload configuration accepted")
	}
	o.passwordFile = "/private/password"
	if err := o.validate(); err != nil {
		t.Fatal(err)
	}
	o.mode = "rex"
	if err := o.validate(); err == nil {
		t.Fatal("native-only uploads accepted")
	}
}

func TestWebBoundary(t *testing.T) {
	store := workspace.New("/bin/sh", workspace.Sandbox{})
	defer store.Close()
	o := validOptions()
	o.publicURL = "https://terminal.example"
	h := webHandler(o, store, fstest.MapFS{"index.html": {Data: []byte("embedded")}}, "a-long-private-password")
	for _, test := range []struct {
		path, host, origin, password string
		status                       int
	}{
		{"/", "evil.example", "", "a-long-private-password", 403},
		{"/", "terminal.example", "", "", 401},
		{"/", "terminal.example", "", "wrong", 401},
		{"/", "terminal.example", "", "a-long-private-password", 200},
		{"/api/sessions", "terminal.example", "", "a-long-private-password", 200},
		{"/api/terminal", "terminal.example", "http://terminal.example", "a-long-private-password", 403},
		{"/api/terminal", "terminal.example", "https://evil.example", "a-long-private-password", 403},
		{"/api/terminal", "terminal.example", "", "a-long-private-password", 403},
		{"/api/control", "terminal.example", "", "a-long-private-password", 404},
	} {
		r := httptest.NewRequest("GET", test.path, nil)
		r.Host = test.host
		if test.origin != "" {
			r.Header.Set("Origin", test.origin)
		}
		if test.password != "" {
			r.SetBasicAuth("rexweb", test.password)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != test.status {
			t.Errorf("%+v: status=%d body=%s", test, w.Code, w.Body.String())
		}
	}
}
func TestNativeRejectsBrowser(t *testing.T) {
	h := nativeHandler(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(204) }))
	for _, origin := range []string{"", "http://localhost"} {
		r := httptest.NewRequest("GET", "/api/control", nil)
		r.Header.Set("Origin", origin)
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		want := 204
		if origin != "" {
			want = 403
		}
		if w.Code != want {
			t.Fatal(w.Code)
		}
	}
}
func TestPasswordFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "password")
	os.WriteFile(path, []byte("0123456789abcdef\n"), 0600)
	if password, err := readPassword(path); err != nil || password != "0123456789abcdef" {
		t.Fatal(password, err)
	}
	os.Chmod(path, 0644)
	if _, err := readPassword(path); err == nil {
		t.Fatal("public password accepted")
	}
	os.Chmod(path, 0600)
	os.WriteFile(path, []byte("short"), 0600)
	if _, err := readPassword(path); err == nil {
		t.Fatal("short password accepted")
	}
}
