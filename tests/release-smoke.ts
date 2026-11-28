import { mkdtemp, rm } from "node:fs/promises";
import { join, resolve } from "node:path";

function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

assert(process.platform === "linux", "release smoke requires Linux");
const version = process.env.VERSION ?? (await Bun.file("VERSION").text()).trim();
const arch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "amd64" : undefined;
assert(arch, `unsupported release architecture: ${process.arch}`);
const name = `rexweb-${version}-rust-linux-${arch}`;
assert(Bun.spawnSync(["sha256sum", "-c", "SHA256SUMS-rust"], { cwd: "dist", stdout: "inherit" }).exitCode === 0, "checksum mismatch");
const scratch = await mkdtemp("/tmp/rexweb-package.");
try {
  assert(Bun.spawnSync(["tar", "-xzf", resolve("dist", `${name}.tar.gz`), "-C", scratch]).exitCode === 0, "extract package");
  const bundle = join(scratch, name);
  const binary = join(bundle, "bin/rexweb");
  const info = Bun.spawnSync([binary, "-version"]);
  assert(info.exitCode === 0 && info.stdout.toString().startsWith(`rexweb ${version} (`), "incorrect build version");
  const elf = Bun.spawnSync(["readelf", "-l", "-d", binary]);
  assert(elf.exitCode === 0 && !/\bINTERP\b|\(NEEDED\)/.test(elf.stdout.toString()), "package contains a dynamically linked binary");
  for (const path of ["LICENSE", "docs/three-pane-workspace.png", "THIRD_PARTY_NOTICES.md", "licenses/ghostty-web-MIT.txt", "licenses/rust/RUST-DEPENDENCIES.md", "deploy/rexweb.service"]) {
    assert(await Bun.file(join(bundle, path)).exists(), `missing ${path}`);
  }
  assert(!await Bun.file(join(bundle, "web/dist/index.html")).exists(), "frontend was not packaged solely inside executable");
  const test = Bun.spawn(["bun", "tests/rex-smoke.ts", binary], { stdout: "inherit", stderr: "inherit", env: process.env });
  assert(await test.exited === 0, "extracted binary smoke failed");
  console.log(`Release package passed: Linux ${arch}; checksums, version, notices, static executable and full service smoke.`);
} finally {
  await rm(scratch, { recursive: true, force: true });
}
