import { mkdtemp, mkdir, realpath, rm } from "node:fs/promises";
import { join, resolve } from "node:path";

function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

assert(process.platform === "darwin", "release smoke test requires macOS");
const version = (await Bun.file("VERSION").text()).trim();
const arch = process.arch === "arm64" ? "arm64" : "amd64";
const name = `rexweb-${version}-darwin-${arch}`;
const archive = resolve("dist", `${name}.tar.gz`);
const checksums = Bun.spawnSync(["shasum", "-a", "256", "-c", "SHA256SUMS"], { cwd: "dist" });
assert(checksums.exitCode === 0, "release archive checksum mismatch");
const scratch = await realpath(await mkdtemp("/tmp/rexweb-package-test."));
const bundle = join(scratch, name);
const socketDir = join(scratch, "run");
let gateway: ReturnType<typeof Bun.spawn> | undefined;

try {
  assert(Bun.spawnSync(["tar", "-xzf", archive, "-C", scratch]).exitCode === 0, "extract release archive");
  const binary = join(bundle, "bin/rexweb");
  const info = Bun.spawnSync([binary, "-version"]);
  assert(info.exitCode === 0 && new TextDecoder().decode(info.stdout).startsWith(`rexweb ${version} (`),
    "release binary has incorrect version");
  for (const path of ["LICENSE", "THIRD_PARTY_NOTICES.md", "licenses/ghostty-web-MIT.txt",
    "licenses/FiraCode-OFL-1.1.txt", "web/dist/static/js/ghostty-vt.wasm"]) {
    assert(await Bun.file(join(bundle, path)).exists(), `archive missing ${path}`);
  }
  await mkdir(socketDir, { mode: 0o700 });
  const probe = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() });
  const origin = `http://127.0.0.1:${probe.port}`;
  await probe.stop(true);
  gateway = Bun.spawn([binary, "-listen", origin.slice(7), "-session-socket", join(socketDir, "session.sock"),
    "-workspace", join(scratch, "work")], { cwd: bundle, stdout: "ignore", stderr: "inherit" });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    const response = await fetch(origin).catch(() => null);
    if (response?.ok) {
      assert((await response.text()).includes("Rex Workspace"), "packaged frontend missing");
      ready = true;
      break;
    }
    assert(gateway.exitCode === null, "packaged gateway exited during startup");
    await Bun.sleep(100);
  }
  assert(ready, "packaged gateway startup timed out");
  for (const path of ["/static/js/workspace.js", "/static/workspace.css", "/static/THIRD_PARTY_NOTICES.md",
    "/static/licenses/ghostty-web-MIT.txt"]) {
    const response = await fetch(origin + path);
    assert(response.ok && (await response.arrayBuffer()).byteLength > 0, `packaged asset missing: ${path}`);
  }
  const wasm = new Uint8Array(await (await fetch(origin + "/static/js/ghostty-vt.wasm")).arrayBuffer());
  assert(wasm[0] === 0 && wasm[1] === 97 && wasm[2] === 115 && wasm[3] === 109, "packaged WASM invalid");
  const { sessions } = await (await fetch(origin + "/api/sessions")).json();
  assert(sessions.length === 1 && sessions[0].label === "Shell", "packaged default session missing");
  console.log(`Release package smoke passed: ${arch}, checksums, version, assets, licences and default shell.`);
} finally {
  gateway?.kill();
  if (gateway) await gateway.exited;
  const owners = Bun.spawnSync(["/usr/sbin/lsof", "-t", join(socketDir, "daemon.lock")]);
  for (const id of new TextDecoder().decode(owners.stdout).trim().split(/\s+/).filter(Boolean)) {
    const pid = Number(id);
    try { process.kill(pid, "SIGTERM"); } catch { continue; }
    for (let attempt = 0; attempt < 50; attempt++) {
      try { process.kill(pid, 0); } catch { break; }
      await Bun.sleep(50);
    }
  }
  await rm(scratch, { recursive: true, force: true });
}
