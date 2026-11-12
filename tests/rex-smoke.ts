import { mkdtemp, mkdir, writeFile, rm, stat } from "node:fs/promises";
import { resolve, join } from "node:path";
import { smokeNative } from "./rex-native";

const binary = resolve(Bun.argv[2] ?? "bin/rexweb");
const scratch = await mkdtemp("/tmp/rexweb-smoke.");
const services = new Set<ReturnType<typeof Bun.spawn>>();
function assert(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
const quote = (s: string) => "'" + s.replaceAll("'", "'\\''") + "'";
const canary = join(scratch,"canary");
await writeFile(canary,"outside data\n");

async function start(name: string, extra: string[] = []) {
 const dir=join(scratch,name);await mkdir(dir,{mode:0o700});
 const socket=join(dir,"run/session.sock"),work=join(dir,"work");
 const child=Bun.spawn([binary,"-mode","both","-listen","127.0.0.1:0","-rex-listen","127.0.0.1:0","-session-socket",socket,"-workspace",work,...extra],{
  cwd:dir,env:{...process.env,REXWEB_SMOKE_SECRET:"private"},stdout:"pipe",stderr:"pipe",
 });
 services.add(child);
 let logs="";
 const reader=(async()=>{for await(const data of child.stderr) logs+=new TextDecoder().decode(data);})();
 for(let i=0;i<100;i++) {
  if(logs.includes("Ready:")) {
   const web=logs.match(/Web listener at (\S+)/)?.[1];
   const rex=logs.match(/Rex listener at (\S+)/)?.[1];
   assert((await stat(socket)).mode%0o1000===0o600,"Unix socket is not private");
   return {child,origin:web?`http://${web}`:"",rex,socket,work,logs};
  }
  if(child.exitCode!==null) throw new Error("startup failed: "+logs);
  await Bun.sleep(50);
 }
 throw new Error("startup timeout: "+logs);
}
async function stop(s: Awaited<ReturnType<typeof start>>) {
 s.child.kill("SIGTERM");
 const code=await Promise.race([s.child.exited,Bun.sleep(5000).then(()=>{throw new Error("shutdown hung")})]);
 services.delete(s.child);assert(code===0,"unclean service shutdown: "+code);
}
async function api(origin:string,path:string,body?:object) {
 const r=await fetch(origin+path,body?{method:"POST",headers:{Origin:origin,"Content-Type":"application/json"},body:JSON.stringify(body)}:undefined);
 assert(r.ok,`${path}: ${r.status} ${await r.clone().text()}`);return r.json();
}
async function pane(origin:string,session:string,block:string,command:string,marker:string,resume?:number): Promise<number> {
 for(let attempt=0;;attempt++) {
  try {return await paneAttempt(origin,session,block,command,marker,resume)} catch(error) {
   // A client close event can precede server-side viewer detachment. Retry
   // only failed upgrades before any input was sent; never replay a command.
   if(!(error instanceof Error)||error.message!=="PTY connection failed before ready"||attempt>=9)throw error;
   await Bun.sleep(50);
  }
 }
}
async function paneAttempt(origin:string,session:string,block:string,command:string,marker:string,resume?:number) {
 return new Promise<number>((resolve,reject)=>{
  const args=new URLSearchParams({session,block});if(resume!==undefined)args.set("offset",String(resume));
  const ws=new WebSocket(origin.replace("http:","ws:")+"/api/terminal?"+args,{headers:{Origin:origin}});ws.binaryType="arraybuffer";
  let content="",offset=0,failure:Error|undefined,ready=false;
  const timer=setTimeout(()=>{failure=new Error(`PTY timeout waiting for ${marker}: ${content}`);ws.close();reject(failure)},8000);
  ws.onmessage=e=>{
   if(typeof e.data==="string") {
    const [kind,payload]=JSON.parse(e.data);
    if(kind==="stream") {offset=payload.offset;if(resume!==undefined&&(offset!==resume||payload.reset)){failure=new Error("byte-offset resume failed");ws.close();}}
    if(kind==="ready") {ready=true;ws.send(JSON.stringify(["resize",{width:91,height:30}]));ws.send(JSON.stringify(["stdin",command+"\r"]));}
   } else {offset+=e.data.byteLength;content+=new TextDecoder().decode(e.data);if(content.includes(marker))ws.close();}
  };
  ws.onerror=()=>{failure=new Error(ready?"PTY connection failed after ready":"PTY connection failed before ready")};
  ws.onclose=()=>{clearTimeout(timer);if(failure||!content.includes(marker))reject(failure??new Error(content));else resolve(offset)};
 });
}
try {
 const s=await start("both");
 const origin=s.origin;
 assert((await (await fetch(origin)).text()).includes("Rex Workspace"),"embedded index missing");
 for(const path of ["/static/js/workspace.js","/static/workspace.css","/static/fonts/FiraCodeNerdFont-Regular.ttf","/static/licenses/ghostty-web-MIT.txt"]) {
  const r=await fetch(origin+path);assert(r.ok&&(await r.arrayBuffer()).byteLength>0,"embedded asset missing: "+path);
 }
 const wasm=await fetch(origin+"/static/js/ghostty-vt.wasm");assert(wasm.headers.get("content-type")==="application/wasm","WASM MIME incorrect");
 const magic=new Uint8Array(await wasm.arrayBuffer());assert(magic.slice(0,4).join(",")==="0,97,115,109","bad WASM");
 const {session_id:session,initial_windows:[first]}=await api(origin,"/api/action",{op:"create",label:"smoke"});
 const [block]=first.block_ids;
 const act=(op:string,args:object={})=>api(origin,"/api/action",{op,session,...args});
 const view=()=>api(origin,"/api/session?"+new URLSearchParams({session}));
 const {block_ids:[right]}=await act("split",{block,direction:"horizontal"});
 await act("split",{block:right,direction:"vertical"});
 await act("resize-split",{before:block,after:right,direction:"horizontal",ratio:.65});
 assert((await view()).windows[0].layers[0].blocks.length===3,"nested splits missing");
 const tab=await act("new-window");assert((await view()).active_window_id===tab.window_id,"tab missing");
 await act("focus-window",{window:first.window_id});await act("focus-block",{block});
 await act("rename",{label:"renamed"});assert((await view()).label==="renamed","rename failed");
 const checks=[
  `test "$(pwd -P)" = ${quote(s.work)}`,
  `test "$HOME" = ${quote(s.work+"/.home")}`,
  'test -z "$REXWEB_SMOKE_SECRET"',
  'touch "$HOME/allowed"',
  `! cat ${quote(canary)} >/dev/null 2>&1`,
  `! touch ${quote(canary+".write")} 2>/dev/null`,
  `! curl -fsS --noproxy '*' --max-time 1 ${quote(origin)} >/dev/null 2>&1`,
  `! curl -fsS --max-time 1 --unix-socket ${quote(s.socket)} http://localhost/api/control >/dev/null 2>&1`,
  'grep -q "CapEff:.*0000000000000000" /proc/self/status',
  '! mount -o remount,rw /usr 2>/dev/null',
  'test "$(stty size)" = "30 91"',
 ];
 await pane(origin,session,block,checks.join(" && ")+" && printf 'sandbox-%s\\n' enforced","sandbox-enforced");
 assert(await Bun.file(s.work+"/.home/allowed").exists(),"home is not writable");
 assert(!await Bun.file(canary+".write").exists(),"sandbox escaped");
 const offset=await pane(origin,session,block,"export CHECK=alive; printf 'first-%s\\n' \"$CHECK\"","first-alive");
 await Bun.sleep(100);
 await pane(origin,session,block,"printf 'resumed-%s\\n' \"$CHECK\"","resumed-alive",offset);
 for(const headers of [{Origin:"http://evil.example"},{Origin:origin,Host:"evil.example"}]) {
  const r=await fetch(origin+"/api/action",{method:"POST",headers:{...headers,"Content-Type":"application/json"},body:JSON.stringify({op:"destroy",session})});assert(r.status===403,"web boundary bypass");
 }
 await Bun.sleep(100);
 const nativePane=await smokeNative(s.rex!,session,block);
 assert((await view()).windows[0].focused_block_id===nativePane,"native layout not shared with web");
 // The official CLI is optional and is never bundled in rexweb.
 if(process.env.REX_REFERENCE_BIN) {
  for(const endpoint of [s.socket,`http://${s.rex}`]) {
   const run=(args:string[])=>{const r=Bun.spawnSync([process.env.REX_REFERENCE_BIN!,"-S",endpoint,"--autostart=false",...args]);assert(r.exitCode===0,new TextDecoder().decode(r.stderr));return new TextDecoder().decode(r.stdout)};
   assert(JSON.parse(run(["ls","--json"])).sessions.some((x:any)=>x.session_id===session),"official CLI sees different sessions");
   const made=JSON.parse(run(["new","cli-test","--json"]));
   run(["split","-s",made.session_id,"--json"]);
   run(["send","-s",made.session_id,"printf cli-ok"]);
   run(["send-key","-s",made.session_id,"enter"]);
   run(["kill",made.session_id]);
  }
 }
 await act("destroy");
 assert(!(await api(origin,"/api/sessions")).sessions.some((x:any)=>x.session_id===session),"destroyed session listed");
 await stop(s);
 const restarted=await start("restart",["-workspace",s.work]);
 assert((await api(restarted.origin,"/api/sessions")).sessions.length===1,"restart default session missing");await stop(restarted);
 const webOnly=await start("web",["-mode","web"]);assert(!webOnly.rex,"web mode started native TCP");await stop(webOnly);
 const rexOnly=await start("rex",["-mode","rex"]);assert(!rexOnly.origin&&rexOnly.rex,"rex mode started web");await stop(rexOnly);
 const host=await start("host",["-isolation","host"]);
 const {sessions:[hs]}=await api(host.origin,"/api/sessions");const hv=await api(host.origin,"/api/session?"+new URLSearchParams({session:hs.session_id}));
 await pane(host.origin,hs.session_id,hv.windows[0].focused_block_id,`cat ${quote(canary)} && printf 'host-%s\\n' access`,"host-access");await stop(host);
 const secret=join(scratch,"password");await writeFile(secret,"test-password-0123456789",{mode:0o600});
 const remote=await start("auth",["-listen","0.0.0.0:0","-public-url","http://terminal.example","-password-file",secret,"-allow-insecure-http"]);
 const authOrigin=remote.origin.replace("0.0.0.0","127.0.0.1").replace("[::]","127.0.0.1");
 const auth=await fetch(authOrigin,{headers:{Host:"terminal.example"}});assert(auth.status===401,"unauthenticated remote web status: "+auth.status+" "+await auth.text()+" at "+authOrigin);
 const logged=await fetch(authOrigin,{headers:{Host:"terminal.example",Authorization:"Basic "+btoa("rexweb:test-password-0123456789")}});assert(logged.ok,"remote web auth failed");await stop(remote);
 const cert=join(scratch,"cert.pem"),key=join(scratch,"key.pem");
 const certgen=Bun.spawnSync(["openssl","req","-x509","-newkey","rsa:2048","-nodes","-keyout",key,"-out",cert,"-days","1","-subj","/CN=localhost","-addext","subjectAltName=DNS:localhost,IP:127.0.0.1"],{stdout:"pipe",stderr:"pipe"});
 assert(certgen.exitCode===0,"TLS test certificate generation failed");
 const tls=await start("tls",["-password-file",secret,"-tls-cert",cert,"-tls-key",key]);
 const tlsOrigin=tls.origin.replace("http:","https:");
 assert((await fetch(tlsOrigin,{tls:{rejectUnauthorized:false}})).status===401,"TLS web did not require authentication");
 const tlsSessions=await fetch(tlsOrigin+"/api/sessions",{headers:{Authorization:"Basic "+btoa("rexweb:test-password-0123456789")},tls:{rejectUnauthorized:false}});
 assert(tlsSessions.ok&&(await tlsSessions.json()).sessions.length===1,"authenticated TLS request failed");
 if(process.env.REX_REFERENCE_BIN) {
  const r=Bun.spawnSync([process.env.REX_REFERENCE_BIN,"-S",`https://${tls.rex}`,"--autostart=false","ls","--json"],{env:{...process.env,SSL_CERT_FILE:cert}});
  assert(r.exitCode===0,"official CLI native TLS: "+new TextDecoder().decode(r.stderr));
 }
 await stop(tls);
 const bad=Bun.spawnSync([binary,"-mode","rex","-session-socket",join(scratch,"bad/run/session.sock"),"-workspace",join(scratch,"bad/work"),"-shell","/nonexistent-shell"],{stdout:"pipe",stderr:"pipe"});
 assert(bad.exitCode!==0&&new TextDecoder().decode(bad.stderr).includes("start sandboxed shell"),"failed sandbox shell was accepted");
 console.log("Linux smoke passed: embedded assets from empty cwd; namespaces, capabilities, network/file/socket denial; host opt-in; PTY resize and reconnect; nested layouts; native protocol; modes; web auth; TLS web/native; failed-shell rejection; clean shutdown.");
} finally {
 for(const child of services) {child.kill("SIGKILL");await child.exited;}
 await rm(scratch,{recursive:true,force:true});
}
