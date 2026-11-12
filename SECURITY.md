# Security

rexweb is a single-account Linux terminal service. Anyone admitted to it can control all its sessions. It has no tenant separation or per-session authorisation. It has not received an independent security audit.

## Shell boundary

The default namespace sandbox requires an unprivileged Linux account and user namespaces. Each helper creates user, mount, network, PID, IPC and UTS namespaces. It maps that account to namespace UID/GID 0, binds system tools read-only, mounts a PID-namespace-local procfs, exposes minimal devices and the pane's PTY, and binds the private workspace read-write. The rest of the root filesystem is a read-only tmpfs. The helper chroots, sets no-new-privileges, drops bounding/effective/permitted/inheritable capabilities, and execs the shell. No privileged helper or external sandbox executable is needed.

The host home, runtime directory and service socket are absent. Shells receive an allowlisted environment with a workspace-local HOME/TMPDIR. New network namespaces have no configured network. Panes share the workspace and must be mutually trusted. Files or credentials deliberately placed in it are available to all panes. System trees such as `/usr` must not contain secrets. A kernel vulnerability can compromise namespace isolation; keep the host patched. There is no seccomp syscall filter or per-pane resource quota. Systemd resource limits apply to the service as a whole.

Startup probes the sandbox before binding listeners. A per-pane exec-status pipe rejects failed helper setup before admitting the pane. Unsupported namespace/mount policies fail closed. There is no automatic fallback to host mode. `-isolation host` explicitly disables the shell boundary, logs a warning, and grants the service account's full file/network permissions. Its workspace and environment remain private by default, but programs can access that account's other resources.

The service refuses root. Use a dedicated account for shared access. The systemd units use KillMode=control-group, so stopped services also terminate background jobs. Outside systemd, host-mode shutdown closes the PTY and kills its shell; detached descendants can outlive it. Sandbox PID namespaces end when their shell exits.

## Listeners

- The local Unix socket is 0600 in an owner-controlled 0700 directory. Its lifetime lock prevents competing instances from replacing it. The socket exposes the native Rex API; possession of it is service access.
- Web TCP defaults to loopback. Non-loopback web binds require a private password file, explicit external origin and TLS, or explicit HTTP opt-in for a trusted TLS-terminating proxy. HTTP Basic authentication covers static assets, APIs and WebSocket handshakes. Use long random passwords. Password rotation requires a restart.
- Web requests enforce Host; mutations and terminal upgrades also require the exact matching HTTP(S) Origin. Browser terminal routes are allowlisted. Cross-origin native browser requests are rejected.
- Native TCP is opt-in. Non-loopback binds require `-allow-anonymous`. This endpoint has no password authentication and grants all reachable clients service access, even if TLS is enabled. Restrict it with SSH or a trusted VPN/firewall. TLS alone supplies transport encryption, not client authorisation.
- Loopback TCP is accessible to other accounts on the machine. Configure a web password even locally when that matters; use only the private Unix socket for native access on a shared host.

Do not expose an anonymous Rex endpoint to the public internet. Reverse proxies must preserve Host, Origin and Authorization, support WebSocket upgrades and prevent unauthorised bypass. The service never trusts X-Forwarded-* identity headers.

Terminal output is untrusted. Keep the pinned Ghostty security fixes. Do not log terminal input/output, passwords, authentication headers or reconnect/data tickets. Native tickets are one-use, expire after 60 seconds and are bound to a live control connection.
