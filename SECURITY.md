# Security

rexweb is an experimental local terminal service for one operator. It is not intended for public hosting or multiple untrusted users.

The Go gateway and daemon are trusted processes running as your account. A macOS kernel sandbox restricts each shell and its descendants. Shells receive an explicit environment, a workspace outside the account home, and private `HOME` and `TMPDIR` directories. The daemon checks isolation before opening its socket and fails closed on unsupported platforms.

The shell sandbox blocks outbound connections, the daemon socket and file contents outside the permitted workspace and system paths. Panes in the same daemon share the workspace and are not isolated from one another. Anyone who can access the private daemon socket can operate its sessions.

The browser gateway binds to a loopback IP, checks the request Host and requires a matching Origin for mutations and terminal WebSockets. The optional native Rex endpoint is also loopback-only and has no account authentication. Keep both endpoints local. Apple deprecates `sandbox-exec`; changes to macOS policy enforcement need integration testing before a release is supported there.

Treat terminal output as untrusted. Keep the pinned Ghostty fork and its security fixes when updating dependencies. No release has undergone an independent security audit.

For sensitive reports, use the repository's **Security → Report a vulnerability** feature when enabled. Otherwise contact the maintainer through the contact information on their GitHub profile. Include the affected version, macOS version, reproduction steps and impact. Use a public issue for non-sensitive bugs.
