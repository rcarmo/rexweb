# Rex web workspace

Run interactive shell sessions in a Go daemon and manage them in a browser built with webterm's TypeScript terminal component and Ghostty WASM renderer. The workspace provides sessions, tabs, nested splits and resizing. The daemon runs the shells and owns the layout.

## Run locally

Install Go 1.24 or later and Bun on macOS. The session daemon requires macOS with `sandbox-exec`. From the repository root, run:

```sh
make install-dev
make build
bin/rexweb
```

Open [http://127.0.0.1:8080](http://127.0.0.1:8080). The gateway automatically starts a missing session daemon and opens a default shell. Reopening the gateway reuses the running daemon and its sessions. Concurrent launches share one daemon and one default session.

The daemon starts `/bin/sh` in a fresh private workspace under `/private/tmp`. Set `-shell /bin/zsh` or an absolute `-workspace` path outside your account home on the initial gateway launch to choose a shell or reuse workspace files. These options apply when a new daemon starts. The daemon writes its startup log and socket in the private `rexweb` directory under the user's cache directory.

To manage the daemon separately, run `bin/rexweb -serve-sessions` and launch the gateway in another terminal. Pass the same absolute `-session-socket` path to both commands when overriding the default socket. The daemon holds a lifetime lock and removes a stale socket only when its old listener has stopped.

The bundled Superlogical Rex server was used in the original prototype. Existing sessions in that server stay there. The daemon serves the workspace browser API and can also accept native Rex clients on a separate loopback listener.

## Connect Rex Beta.app

On the initial launch, enable the optional Rex listener:

```sh
bin/rexweb -rex-listen 127.0.0.1:7297
```

In Rex Beta.app choose **Add Remote Host…** and enter `http://127.0.0.1:7297`. The app can create and attach to shell sessions, manage tabs and tiled splits, and send terminal input. The web gateway can use the same daemon. Each pane accepts one live terminal connection at a time. Switch to another pane or close the current viewer before opening that pane in the other client.

This listener implements the tested subset of Rex control API revision 12 and the ticketed `rex.data.terminal.v1` stream used by Rex Beta build 1925. It sends a blank Ghostty version-1 snapshot followed by available PTY history and live output. Reopening a stream uses a fresh ticket. The listener accepts local clients without account authentication and is off by default. The paste RPC passes the wire smoke test. Command-V in the native app did not send that request during testing. Lua configuration, floating panes, remote authentication, the full automation API and native stream resume are outside this subset. [Rex Server API](https://www.superlogical.com/rex/docs/automate/server-api) · [Adding a remote host](https://www.superlogical.com/rex/docs/linux/remote-access#adding-the-server-to-a-client)

## Sessions and panes

- List, select, create, rename and end sessions. Show each session's windows as tabs. Each window has a binary tree of horizontal or vertical splits and one focused pane.
- Create and focus tabs and panes. Resize splits with a pointer or the arrow keys. Closing a pane ends its shell. Ending a session closes all its panes.
- Use a pseudo-terminal (PTY) for each shell. Forward input and cell dimensions to the PTY and stream output as binary WebSocket frames.
- Keep terminal output and layout in the daemon while it runs. Browser disconnects and gateway restarts leave shells alive. Stopping the daemon ends its shells and discards the in-memory workspace.
- Allow one active browser connection per pane. An exited shell keeps its retained output visible until the pane is closed.

## Stream and reconnect

On attachment, the daemon sends `stream` metadata with the starting byte offset and whether the renderer needs to reset. It sends retained output before `ready`, then streams live bytes. A reconnect requests the offset immediately after the last byte Ghostty consumed. The daemon sends only bytes after that offset when they are still retained. The browser suppresses input and terminal query replies while replaying history.

The daemon retains 512 KiB of recent PTY output per pane. A fresh attachment or a reconnect outside that range rebuilds the renderer from the available bytes. Output older than the retained buffer is lost to a new renderer, and a truncated replay may lack earlier terminal modes or screen state. Slow connections detach when their output queue fills. Browser input is discarded on disconnect.

## Shell isolation

The Go daemon allocates PTYs and runs each shell under a macOS kernel sandbox. The shell and its subprocesses can write to the workspace, with `HOME` set to `.home` and `TMPDIR` set to `.tmp` inside that workspace. The sandbox grants reads of system tools and libraries and access to the pane's own terminal device. It blocks file contents outside those paths, outbound network connections and access to the daemon socket. Shells receive an explicit environment instead of the daemon's environment.

The Go daemon and web gateway run as trusted local processes. The sandbox applies at the shell boundary. The daemon verifies the policy before opening its socket and refuses startup if the policy cannot be applied. The session daemon currently supports macOS. Apple marks `sandbox-exec` deprecated. [Apple sandbox discussion](https://developer.apple.com/forums/thread/661939)

## Access

The gateway listens on loopback and rejects requests with non-loopback Host values. Mutations and terminal WebSockets require a matching browser Origin. The gateway proxies the allowlisted workspace routes over a private Unix socket. Anyone with access to the daemon socket can run commands as the daemon's account. Treat the local workspace as a single-operator service.

## Verification

Run `make check`, `make race` and `make smoke`. The macOS smoke test checks sandboxed working directories, private shell environments, file and socket access, and symlink escapes. It also exercises nested splits, PTY resizing, byte-offset resume across a gateway restart and revision-12 control and terminal streams. It also verifies automatic startup, a single default shell across concurrent gateways, and recovery after an ungraceful daemon exit.

The frontend uses Bun and webterm's terminal component, fonts and styles. It pins the Ghostty fork to the verified security update. [webterm source](https://github.com/rcarmo/webterm/tree/fe3cc0340b5f4e86ec73ae5dcf311d78930134ec) · [ghostty-web security update](https://github.com/rcarmo/ghostty-web/commit/7ff18ee7b6b42b09da1f5e0553534319d4057cd4)
