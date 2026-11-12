// rexweb is a foreground, single-account Linux terminal service.
package main

import (
	"context"
	"crypto/sha256"
	"crypto/subtle"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"time"

	"rexweb/internal/workspace"
	"rexweb/web"
)

var version = "dev"
var commit = "unknown"

type options struct {
	mode, listen, rexListen, socket, shell, workdir, webdir, publicURL, passwordFile, cert, key, isolation string
	anonymous, insecure                                                                                    bool
}

func main() {
	if len(os.Args) > 1 && os.Args[1] == "__sandbox" {
		if err := workspace.RunSandbox(os.Args[2:]); err != nil {
			fmt.Fprintln(os.Stderr, "sandbox:", err)
			if len(os.Args) > 3 && os.Args[3] != "/dev/null" {
				if f := os.NewFile(3, "sandbox-status"); f != nil {
					fmt.Fprintln(f, err)
					f.Close()
				}
			}
			os.Exit(1)
		}
		return
	}
	var o options
	cache, _ := os.UserCacheDir()
	run := os.Getenv("XDG_RUNTIME_DIR")
	if run == "" {
		run = cache
	}
	flag.StringVar(&o.mode, "mode", "web", "listeners: web, rex, or both (local Rex socket always enabled)")
	flag.StringVar(&o.listen, "listen", "127.0.0.1:8080", "web IP:port")
	flag.StringVar(&o.rexListen, "rex-listen", "127.0.0.1:7297", "native Rex IP:port, enabled in rex/both mode")
	flag.StringVar(&o.socket, "session-socket", filepath.Join(run, "rexweb", "session.sock"), "absolute private local Rex Unix socket")
	flag.StringVar(&o.shell, "shell", "/bin/sh", "absolute shell executable")
	flag.StringVar(&o.workdir, "workspace", "", "private workspace outside account home (default: temporary directory)")
	flag.StringVar(&o.isolation, "isolation", "sandbox", "sandbox (Linux namespaces) or host (explicit full account access)")
	flag.StringVar(&o.webdir, "web-dir", "", "optional development asset directory (default: embedded frontend)")
	flag.StringVar(&o.publicURL, "public-url", "", "exact external web origin, required for non-loopback web listeners")
	flag.StringVar(&o.passwordFile, "password-file", "", "private file containing web HTTP Basic password (username: rexweb)")
	flag.StringVar(&o.cert, "tls-cert", "", "PEM TLS certificate for TCP listeners")
	flag.StringVar(&o.key, "tls-key", "", "PEM TLS key for TCP listeners")
	flag.BoolVar(&o.anonymous, "allow-anonymous", false, "allow unauthenticated non-loopback native Rex access: FULL ACCOUNT ACCESS")
	flag.BoolVar(&o.insecure, "allow-insecure-http", false, "allow remote web password over HTTP (only behind a trusted TLS tunnel)")
	showVersion := flag.Bool("version", false, "print version and exit")
	flag.Parse()
	if *showVersion {
		fmt.Printf("rexweb %s (%s; %s/%s)\n", version, commit, runtime.GOOS, runtime.GOARCH)
		return
	}
	if flag.NArg() != 0 {
		log.Fatal("unexpected positional arguments")
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := runService(ctx, o); err != nil {
		log.Fatal(err)
	}
}

func (o options) validate() error {
	if runtime.GOOS != "linux" {
		return errors.New("this service requires Linux")
	}
	if o.mode != "web" && o.mode != "rex" && o.mode != "both" {
		return errors.New("-mode must be web, rex, or both")
	}
	if o.isolation != "sandbox" && o.isolation != "host" {
		return errors.New("-isolation must be sandbox or host")
	}
	if !filepath.IsAbs(o.shell) || !filepath.IsAbs(o.socket) {
		return errors.New("absolute -shell and -session-socket paths required")
	}
	if (o.cert == "") != (o.key == "") {
		return errors.New("-tls-cert and -tls-key must be supplied together")
	}
	if o.mode != "rex" {
		host, _, err := net.SplitHostPort(o.listen)
		if err != nil {
			return err
		}
		if !net.ParseIP(host).IsLoopback() {
			if o.passwordFile == "" || o.publicURL == "" {
				return errors.New("remote web requires -password-file and -public-url")
			}
			if o.cert == "" && !o.insecure {
				return errors.New("remote web requires TLS or explicit -allow-insecure-http behind a trusted TLS tunnel")
			}
		}
		if o.publicURL != "" {
			u, err := url.Parse(o.publicURL)
			if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" || (u.Path != "" && u.Path != "/") {
				return errors.New("-public-url must be an HTTP(S) origin without path, credentials or query")
			}
			if u.Scheme == "http" && o.passwordFile != "" && o.cert != "" {
				return errors.New("TLS listener requires HTTPS public URL")
			}
		}
	}
	if o.mode != "web" {
		host, _, err := net.SplitHostPort(o.rexListen)
		if err != nil {
			return err
		}
		if !net.ParseIP(host).IsLoopback() && !o.anonymous {
			return errors.New("non-loopback Rex requires explicit -allow-anonymous; prefer SSH forwarding of the private Unix socket")
		}
	}
	return nil
}

func readPassword(path string) (string, error) {
	if path == "" {
		return "", nil
	}
	info, err := os.Lstat(path)
	if err != nil {
		return "", err
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 || info.Size() > 4096 {
		return "", errors.New("password file must be a private regular file (0600), at most 4096 bytes")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	password := strings.TrimRight(string(data), "\r\n")
	if len(password) < 16 {
		return "", errors.New("web password must have at least 16 characters")
	}
	return password, nil
}

func webHandler(o options, store *workspace.Store, assets fs.FS, password string) http.Handler {
	mux := http.NewServeMux()
	api := store.Handler()
	mux.Handle("GET /api/sessions", api)
	mux.Handle("GET /api/session", api)
	mux.Handle("POST /api/action", requireOrigin(api))
	mux.Handle("GET /api/terminal", requireOrigin(api))
	mux.Handle("GET /", http.FileServer(http.FS(assets)))
	expected := sha256.Sum256([]byte(password))
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Content-Type-Options", "nosniff")
		w.Header().Set("X-Frame-Options", "DENY")
		w.Header().Set("Referrer-Policy", "no-referrer")
		w.Header().Set("Cache-Control", "no-store")
		expectedOrigin := "http://" + r.Host
		if r.TLS != nil {
			expectedOrigin = "https://" + r.Host
		}
		if o.publicURL != "" {
			expectedOrigin = strings.TrimSuffix(o.publicURL, "/")
		}
		if origin := r.Header.Get("Origin"); origin != "" && origin != expectedOrigin {
			http.Error(w, "same-origin request required", 403)
			return
		}
		validHost := loopbackHost(r.Host)
		if o.publicURL != "" {
			u, _ := url.Parse(o.publicURL)
			validHost = strings.EqualFold(r.Host, u.Host)
		}
		if !validHost {
			http.Error(w, "unexpected Host", 403)
			return
		}
		if password != "" {
			user, pass, ok := r.BasicAuth()
			got := sha256.Sum256([]byte(pass))
			if !ok || user != "rexweb" || subtle.ConstantTimeCompare(got[:], expected[:]) != 1 {
				w.Header().Set("WWW-Authenticate", `Basic realm="rexweb", charset="UTF-8"`)
				http.Error(w, "authentication required", 401)
				return
			}
		}
		mux.ServeHTTP(w, r)
	})
}

func nativeHandler(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Origin") != "" {
			http.Error(w, "native Rex client required", 403)
			return
		}
		next.ServeHTTP(w, r)
	})
}

func runService(ctx context.Context, o options) error {
	if err := o.validate(); err != nil {
		return err
	}
	password, err := readPassword(o.passwordFile)
	if err != nil {
		return err
	}
	dir, err := prepareSocketDir(o.socket)
	if err != nil {
		return err
	}
	lock, err := lockInstance(dir)
	if err != nil {
		return err
	}
	defer lock.Close()
	env, err := workspace.PrepareEnvironment(o.workdir, dir, o.isolation == "host")
	if err != nil {
		return err
	}
	if o.isolation == "host" {
		log.Print("WARNING: host isolation disabled; shells have full service-account file and network access")
	}
	store := workspace.New(o.shell, env)
	defer store.Close()
	if err := store.EnsureDefault(); err != nil {
		return fmt.Errorf("default shell: %w", err)
	}
	if err := removeStaleSocket(o.socket); err != nil {
		return err
	}
	native := nativeHandler(store.RexHandler())
	local, err := net.Listen("unix", o.socket)
	if err != nil {
		return err
	}
	defer local.Close()
	if err := os.Chmod(o.socket, 0600); err != nil {
		return err
	}
	type endpoint struct {
		listener net.Listener
		handler  http.Handler
		tls      bool
	}
	endpoints := []endpoint{{local, native, false}}
	defer func() {
		for _, e := range endpoints {
			e.listener.Close()
		}
	}()
	if o.mode != "rex" {
		assets := web.Assets()
		if o.webdir != "" {
			assets = os.DirFS(o.webdir)
		}
		if _, err := fs.Stat(assets, "index.html"); err != nil {
			return fmt.Errorf("frontend: %w", err)
		}
		l, err := net.Listen("tcp", o.listen)
		if err != nil {
			return err
		}
		endpoints = append(endpoints, endpoint{l, webHandler(o, store, assets, password), o.cert != ""})
		log.Printf("Web listener at %s", l.Addr())
	}
	if o.mode != "web" {
		l, err := net.Listen("tcp", o.rexListen)
		if err != nil {
			return err
		}
		endpoints = append(endpoints, endpoint{l, native, o.cert != ""})
		log.Printf("Rex listener at %s", l.Addr())
	}
	// net/http does not close hijacked WebSockets. Track them explicitly.
	var mu sync.Mutex
	connections := map[net.Conn]bool{}
	servers := make([]*http.Server, 0, len(endpoints))
	result := make(chan error, len(endpoints))
	for _, e := range endpoints {
		server := newHTTPServer(e.handler)
		tracked := &trackedListener{Listener: e.listener, mu: &mu, connections: connections}
		servers = append(servers, server)
		go func() {
			var err error
			if e.tls {
				err = server.ServeTLS(tracked, o.cert, o.key)
			} else {
				err = server.Serve(tracked)
			}
			result <- err
		}()
	}
	log.Printf("Ready: local Rex socket %s; workspace %s; isolation %s", o.socket, env.Root(), o.isolation)
	select {
	case <-ctx.Done():
		err = nil
	case err = <-result:
		if errors.Is(err, http.ErrServerClosed) {
			err = nil
		}
	}
	for _, s := range servers {
		s.Close()
	}
	mu.Lock()
	active := make([]net.Conn, 0, len(connections))
	for c := range connections {
		active = append(active, c)
	}
	mu.Unlock()
	for _, c := range active {
		c.Close()
	}
	return err
}

func newHTTPServer(handler http.Handler) *http.Server {
	return &http.Server{Handler: handler, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 10 * time.Second, WriteTimeout: 15 * time.Second, IdleTimeout: 60 * time.Second}
}
func loopbackHost(host string) bool {
	if name, _, err := net.SplitHostPort(host); err == nil {
		host = name
	}
	return host == "localhost" || net.ParseIP(strings.Trim(host, "[]")).IsLoopback()
}
func requireOrigin(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		origin, err := url.Parse(r.Header.Get("Origin"))
		if err != nil || (origin.Scheme != "http" && origin.Scheme != "https") || origin.User != nil || origin.Path != "" || origin.RawQuery != "" || origin.Fragment != "" || !strings.EqualFold(origin.Host, r.Host) {
			http.Error(w, "same-origin request required", 403)
			return
		}
		next.ServeHTTP(w, r)
	})
}

// Close runs for both normal HTTP connections and hijacked WebSockets.
type trackedListener struct {
	net.Listener
	mu          *sync.Mutex
	connections map[net.Conn]bool
}
type trackedConn struct {
	net.Conn
	owner *trackedListener
}

func (l *trackedListener) Accept() (net.Conn, error) {
	c, err := l.Listener.Accept()
	if err != nil {
		return nil, err
	}
	t := &trackedConn{c, l}
	l.mu.Lock()
	l.connections[t] = true
	l.mu.Unlock()
	return t, nil
}
func (c *trackedConn) Close() error {
	err := c.Conn.Close()
	c.owner.mu.Lock()
	delete(c.owner.connections, c)
	c.owner.mu.Unlock()
	return err
}
