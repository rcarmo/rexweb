package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"log"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"time"

	"rexweb/internal/workspace"
)

var version = "dev"
var commit = "unknown"

func main() {
	cache, _ := os.UserCacheDir()
	defaultSocket := filepath.Join(cache, "rexweb", "session.sock")
	socket := flag.String("session-socket", defaultSocket, "absolute path to the private session socket")
	daemon := flag.Bool("serve-sessions", false, "run the PTY and session daemon")
	shell := flag.String("shell", "/bin/sh", "absolute path to the sandboxed shell")
	workdir := flag.String("workspace", "", "private workspace directory outside the account home (default: fresh temporary directory)")
	rexListen := flag.String("rex-listen", "", "optional Rex protocol listener on a loopback IP and port")
	listen := flag.String("listen", "127.0.0.1:8080", "loopback IP and HTTP port")
	web := flag.String("web-dir", "web/dist", "Bun build output directory")
	showVersion := flag.Bool("version", false, "print version and exit")
	flag.Parse()
	if *showVersion {
		fmt.Printf("rexweb %s (%s; %s/%s)\n", version, commit, runtime.GOOS, runtime.GOARCH)
		return
	}

	if !filepath.IsAbs(*socket) {
		log.Fatal("specify an absolute -session-socket path")
	}
	var err error
	if *daemon {
		if !filepath.IsAbs(*shell) {
			log.Fatal("specify an absolute -shell path")
		}
		err = serveSessions(*socket, *shell, *workdir, *rexListen)
	} else {
		err = serveGateway(daemonOptions{socket: *socket, shell: *shell, workdir: *workdir, rexListen: *rexListen}, *listen, *web)
	}
	if err != nil {
		log.Fatal(err)
	}
}

func serveSessions(socket, shell, workdir, rexListen string) error {
	dir, err := prepareSocketDir(socket)
	if err != nil {
		return err
	}
	lock, err := lockDaemon(dir)
	if err != nil {
		return err
	}
	defer lock.Close()
	sandbox, err := workspace.PrepareSandbox(workdir, dir)
	if err != nil {
		return err
	}
	if err := removeStaleSocket(socket); err != nil {
		return err
	}
	listener, err := net.Listen("unix", socket)
	if err != nil {
		return err
	}
	defer listener.Close()
	if err := os.Chmod(socket, 0600); err != nil {
		return err
	}

	store := workspace.New(shell, sandbox)
	defer store.Close()
	if err := store.EnsureDefault(); err != nil {
		return fmt.Errorf("start the default shell: %w", err)
	}
	server := newHTTPServer(store.Handler())
	var rexServer *http.Server
	if rexListen != "" {
		host, _, err := net.SplitHostPort(rexListen)
		if err != nil || !net.ParseIP(host).IsLoopback() {
			return fmt.Errorf("-rex-listen requires a loopback IP and port")
		}
		native, err := net.Listen("tcp", rexListen)
		if err != nil {
			return err
		}
		defer native.Close()
		handler := store.RexHandler()
		rexServer = newHTTPServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if !loopbackHost(r.Host) || r.Header.Get("Origin") != "" {
				http.Error(w, "local Rex client required", http.StatusForbidden)
				return
			}
			handler.ServeHTTP(w, r)
		}))
		go func() {
			if err := rexServer.Serve(native); err != nil && !errors.Is(err, http.ErrServerClosed) {
				log.Printf("Rex listener: %v", err)
			}
		}()
		log.Printf("Rex protocol at http://%s", native.Addr())
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	go func() {
		<-ctx.Done()
		if rexServer != nil {
			rexServer.Close()
		}
		server.Close()
	}()
	log.Printf("Session daemon on %s, sandboxed shell workspace %s", socket, sandbox.Root())
	if err := server.Serve(listener); !errors.Is(err, http.ErrServerClosed) {
		return err
	}
	return nil
}

func serveGateway(options daemonOptions, listen, web string) error {
	host, _, err := net.SplitHostPort(listen)
	if err != nil || !net.ParseIP(host).IsLoopback() {
		return fmt.Errorf("specify a loopback IP and HTTP port")
	}
	if _, err := os.Stat(filepath.Join(web, "index.html")); err != nil {
		return fmt.Errorf("build the frontend with make build: %w", err)
	}
	if err := ensureDaemon(options); err != nil {
		return err
	}

	proxy := &httputil.ReverseProxy{
		Rewrite: func(req *httputil.ProxyRequest) {
			req.SetURL(&url.URL{Scheme: "http", Host: "localhost"})
		},
		Transport: &http.Transport{
			DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
				return (&net.Dialer{Timeout: 5 * time.Second}).DialContext(ctx, "unix", options.socket)
			},
			ResponseHeaderTimeout: 10 * time.Second,
			IdleConnTimeout:       60 * time.Second,
		},
		ErrorHandler: func(w http.ResponseWriter, _ *http.Request, _ error) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadGateway)
			fmt.Fprintln(w, `{"message":"Session daemon unavailable. Restart bin/rexweb to reconnect."}`)
		},
	}
	mux := http.NewServeMux()
	mux.Handle("GET /api/sessions", proxy)
	mux.Handle("GET /api/session", proxy)
	mux.Handle("POST /api/action", requireOrigin(proxy))
	mux.Handle("GET /api/terminal", requireOrigin(proxy))
	mux.Handle("GET /", http.FileServer(http.Dir(web)))

	server := newHTTPServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !loopbackHost(r.Host) {
			http.Error(w, "loopback Host required", http.StatusForbidden)
			return
		}
		w.Header().Set("X-Content-Type-Options", "nosniff")
		w.Header().Set("X-Frame-Options", "DENY")
		mux.ServeHTTP(w, r)
	}))
	server.Addr = listen
	log.Printf("Rex web client at http://%s", listen)
	return server.ListenAndServe()
}

func newHTTPServer(handler http.Handler) *http.Server {
	return &http.Server{
		Handler:           handler,
		ReadHeaderTimeout: 5 * time.Second,
		ReadTimeout:       10 * time.Second,
		WriteTimeout:      15 * time.Second,
		IdleTimeout:       60 * time.Second,
	}
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
		if err != nil || origin.Scheme != "http" || !strings.EqualFold(origin.Host, r.Host) {
			http.Error(w, "same-origin request required", http.StatusForbidden)
			return
		}
		next.ServeHTTP(w, r)
	})
}
