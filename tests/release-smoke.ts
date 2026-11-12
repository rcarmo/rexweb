import { mkdtemp, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
function assert(value:unknown,message:string):asserts value {if(!value)throw new Error(message)}
assert(process.platform==="linux","release smoke requires Linux");
const version=(await Bun.file("VERSION").text()).trim();
const arch=process.arch==="arm64"?"arm64":"amd64";
const name=`rexweb-${version}-linux-${arch}`;
assert(Bun.spawnSync(["sha256sum","-c","SHA256SUMS"],{cwd:"dist"}).exitCode===0,"checksum mismatch");
const scratch=await mkdtemp("/tmp/rexweb-package.");
try {
 assert(Bun.spawnSync(["tar","-xzf",resolve("dist",name+".tar.gz"),"-C",scratch]).exitCode===0,"extract package");
 const bundle=join(scratch,name),binary=join(bundle,"bin/rexweb");
 const info=Bun.spawnSync([binary,"-version"]);
 assert(info.exitCode===0&&new TextDecoder().decode(info.stdout).startsWith(`rexweb ${version} (`),"incorrect build version");
 for(const path of ["LICENSE","THIRD_PARTY_NOTICES.md","licenses/ghostty-web-MIT.txt","deploy/rexweb.service"]) assert(await Bun.file(join(bundle,path)).exists(),"missing "+path);
 assert(!await Bun.file(join(bundle,"web/dist/index.html")).exists(),"frontend was not packaged solely inside executable");
 const test=Bun.spawn(["bun","tests/rex-smoke.ts",binary],{stdout:"inherit",stderr:"inherit",env:process.env});
 assert(await test.exited===0,"extracted binary smoke failed");
 console.log(`Release package passed: Linux ${arch}; checksums, version, notices, standalone binary and full service smoke.`);
} finally {await rm(scratch,{recursive:true,force:true})}
