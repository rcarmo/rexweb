import { checkReplay } from "./replay-browser-checks";
import { checkRenderer } from "./renderer-browser-checks";
import { chromium } from "../web/node_modules/playwright";
import { mkdtemp, rm } from "node:fs/promises";
import { resolve, join } from "node:path";

const scratch=await mkdtemp("/tmp/rexweb-browser.");
const server=Bun.spawn([resolve("bin/rexweb"),"-mode","web","-listen","127.0.0.1:0","-session-socket",join(scratch,"run/session.sock"),"-workspace",join(scratch,"work")],{stdout:"pipe",stderr:"pipe"});
let logs="";void(async()=>{for await(const data of server.stderr)logs+=new TextDecoder().decode(data)})();
const shared=Boolean(process.env.BROWSER_CDP_URL);
const browser=await (async()=>{
 try {return shared?await chromium.connectOverCDP(process.env.BROWSER_CDP_URL!):await chromium.launch()}
 catch(error){server.kill();await server.exited;await rm(scratch,{recursive:true,force:true});throw error}
})();
const context=shared?browser.contexts()[0]:await browser.newContext({hasTouch:true,isMobile:true,viewport:{width:800,height:900}});
const page=await context.newPage();
function assert(value:unknown,message:string):asserts value{if(!value)throw new Error(message)}
const frames:{block:string;data:string}[]=[];
const dimensions=new Map<string,{width:number;height:number}>();
const errors:string[]=[];
page.on("pageerror",e=>errors.push(String(e)));
page.on("websocket",socket=>{
 const block=new URL(socket.url()).searchParams.get("block")??"";
 socket.on("framesent",e=>{try{const [kind,data]=JSON.parse(String(e.payload));if(kind==="stdin")frames.push({block,data});else if(kind==="resize")dimensions.set(block,data)}catch{}});
});
async function helperCount(count=1) {
 assert(await page.locator(".mobile-keybar").count()===count,"duplicate/missing keyboard toolbar");
 assert(await page.locator("[data-mobile-keyboard-input]").count()===count,"duplicate/missing keyboard input");
 assert(await page.locator("style[data-mobile-keyboard-style]").count()===count,"duplicate/missing keyboard stylesheet");
}
async function typeMobile(data:string) {
 await page.locator("[data-mobile-keyboard-input]").evaluate((input,data)=>input.dispatchEvent(new InputEvent("beforeinput",{inputType:"insertText",data,bubbles:true,cancelable:true})),data);
 await page.waitForTimeout(100);
}
try {
 for(let i=0;i<100&&!logs.includes("Ready:");i++){assert(server.exitCode===null,"server exited: "+logs);await Bun.sleep(50)}
 const address=logs.match(/Web listener at (\S+)/)?.[1];assert(address,"no listener: "+logs);
 const cdp=await context.newCDPSession(page);await cdp.send("Emulation.setTouchEmulationEnabled",{enabled:true,maxTouchPoints:1});
 await page.goto(`http://${address}`);
 await page.waitForSelector(".webterm-terminal.-connected");await helperCount();
 await page.locator(".mobile-keybar").evaluate(el=>{el.dataset.testIdentity="original"});
 await page.getByRole("button",{name:"Split right",exact:true}).first().click();
 await page.waitForFunction(()=>document.querySelectorAll(".webterm-terminal.-connected").length===2);await helperCount();
 const panes=page.locator(".pane");
 // Search belongs to the focused pane, not every mounted renderer.
 await panes.nth(0).locator(".webterm-terminal").click();
 await typeMobile("printf 'SEARCH-%s\\n' LEFTONLY\r");
 await panes.nth(1).locator(".webterm-terminal").click();
 await typeMobile("printf 'SEARCH-%s\\n' RIGHTONLY\r");
 await page.getByRole("button",{name:"Find in pane",exact:true}).click();
 await page.getByRole("searchbox",{name:"Find in focused pane",exact:true}).fill("SEARCH-LEFTONLY");
 await page.waitForTimeout(200);
 assert(await page.locator("#search-results").textContent()==="0 matches","search leaked from the other pane");
 await panes.nth(0).locator(".webterm-terminal").click();
 await page.waitForFunction(()=>document.querySelector("#search-results")?.textContent==="1 / 1");
 await page.getByRole("searchbox",{name:"Find in focused pane",exact:true}).focus();
 await page.keyboard.press("Escape");
 assert(await page.locator("#search-bar").isHidden(),"Escape did not close search");
 await page.keyboard.press("Control+f");
 assert(await page.locator("#search-text").evaluate(el=>document.activeElement===el),"Ctrl+F did not focus terminal search");
 await page.keyboard.press("Escape");
 // The singleton input moves between panes rather than leaving IME at (0,0).
 await panes.nth(1).locator(".webterm-terminal").click();
 const anchored=await page.locator("[data-mobile-keyboard-input]").boundingBox();
 const targetBox=await panes.nth(1).locator(".webterm-terminal").boundingBox();
 assert(anchored&&targetBox&&anchored.x>=targetBox.x&&anchored.x<targetBox.x+targetBox.width&&anchored.y>=targetBox.y&&anchored.y<targetBox.y+targetBox.height,"shared IME input is outside the focused pane");
 // Native Copy stages only the current selection in the one shared input.
 // Stub clipboard writers during selection so the test never replaces the
 // operator's system clipboard; inspect the native copy event's payload.
 await panes.nth(0).locator(".webterm-terminal").click();
 const copyBlock=frames.find(f=>f.data.includes('LEFTONLY'))?.block;
 const cells=copyBlock?dimensions.get(copyBlock):undefined;
 const copyCanvas=panes.nth(0).locator('.webterm-terminal canvas').first();
 const copyBox=await copyCanvas.boundingBox();
 assert(cells&&copyBox,'missing terminal cell dimensions');
 await copyCanvas.evaluate((canvas,point)=>{
  const clipboard=navigator.clipboard;
  const write=clipboard?.write,writeText=clipboard?.writeText;
  try{
   if(clipboard){clipboard.write=async()=>{};clipboard.writeText=async()=>{};}
   canvas.dispatchEvent(new MouseEvent('click',{detail:2,clientX:point.x,clientY:point.y,bubbles:true,cancelable:true}));
  }finally{if(clipboard){clipboard.write=write;clipboard.writeText=writeText;}}
 },{x:copyBox.x+copyBox.width/cells.width*5.5,y:copyBox.y+copyBox.height/cells.height*1.5});
 await panes.nth(0).locator('.webterm-terminal').dispatchEvent('contextmenu');
 const copyInput=page.locator('[data-mobile-keyboard-input]');
 const staged=await copyInput.inputValue();
 assert(staged==='SEARCH-LEFTONLY','native Copy did not stage the terminal selection: '+JSON.stringify(staged));
 const copied=await copyInput.evaluate(input=>{
  const data=new DataTransfer();input.dispatchEvent(new ClipboardEvent('copy',{clipboardData:data,bubbles:true,cancelable:true}));return data.getData('text/plain');
 });
 assert(copied===staged&&await copyInput.inputValue()==='','native Copy lost or retained staged clipboard text');
 await panes.nth(0).locator(".webterm-terminal").click();
 const beforeInput=await copyInput.boundingBox();
 await typeMobile("LEFT-TOKEN");
 const afterInput=await copyInput.boundingBox();
 assert(beforeInput&&afterInput&&afterInput.x>beforeInput.x,'IME did not follow horizontal cursor movement');

 const left=frames.find(f=>f.data==="LEFT-TOKEN")?.block;assert(left,"left pane received no input");
 await panes.nth(1).locator(".webterm-terminal").click();
 await page.locator('[data-modifier="ctrl"]').dispatchEvent("pointerdown");
 await typeMobile("c");
 const right=frames.find(f=>f.data==="\x03")?.block;assert(right&&right!==left,"Ctrl input was not routed to the right pane");
 assert(await page.locator('[data-modifier="ctrl"]').getAttribute("class")!=="active","modifier did not reset");
 // A modifier selected on one pane must not leak to another pane.
 await page.locator('[data-modifier="ctrl"]').dispatchEvent("pointerdown");
 await panes.nth(0).locator(".webterm-terminal").click();await typeMobile("x");
 assert(frames.some(f=>f.block===left&&f.data==="x"),"modifier leaked across focus change");
 // Paste must use Ghostty's sanitisation and bracketed-paste mode, and
 // duplicate paste/beforeinput notifications must not type text twice.
 await typeMobile("\x03");
 await typeMobile("printf '\\033[?2004h'\r");
 await page.waitForTimeout(300);
 const pasted = "paste-λ\nsecond\x1b[201~";
 let from = frames.length;
 await page.locator("[data-mobile-keyboard-input]").evaluate((input,text)=>{
  const data=new DataTransfer();data.setData("text/plain",text);
  input.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,bubbles:true,cancelable:true}));
  input.dispatchEvent(new InputEvent("beforeinput",{inputType:"insertFromPaste",data:text,bubbles:true,cancelable:true}));
 },pasted);
 await page.waitForTimeout(100);
 let sent=frames.slice(from);
 assert(sent.length===1&&sent[0].block===left&&sent[0].data==="\x1b[200~paste-λ\nsecond [201~\x1b[201~","mobile bracketed paste was corrupted or duplicated: "+JSON.stringify(sent));
 // The beforeinput-only path is used by some mobile browsers.
 from=frames.length;
 await page.locator("[data-mobile-keyboard-input]").evaluate(input=>input.dispatchEvent(new InputEvent("beforeinput",{inputType:"insertFromPaste",data:"fallback-paste",bubbles:true,cancelable:true})));
 await page.waitForTimeout(100);
 assert(frames.slice(from).map(f=>f.data).join("")==="\x1b[200~fallback-paste\x1b[201~","mobile paste fallback failed: "+JSON.stringify(frames.slice(from)));
 // Large Unicode pastes remain ordered and below the server's message limit.
 const large="a".repeat(8191)+"😀"+"λ".repeat(70000);
 const wrapped="\x1b[200~"+large+"\x1b[201~";
 const bytes=new TextEncoder().encode(wrapped).length;
 await panes.nth(1).locator(".webterm-terminal").click();
 await typeMobile(`printf '\\033[?2004h'; stty -echo -icanon min 1 time 0; head -c ${bytes} > "$TMPDIR/paste-check"; stty echo icanon; printf '\\033[?2004l'\r`);
 await page.waitForTimeout(300);
 from=frames.length;
 await page.locator("[data-mobile-keyboard-input]").evaluate((input,text)=>{
  const data=new DataTransfer();data.setData("text/plain",text);
  input.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,bubbles:true,cancelable:true}));
 },large);
 await page.waitForTimeout(500);
 sent=frames.slice(from);
 assert(sent.map(f=>f.data).join("")===wrapped,"large Unicode paste lost bytes");
 for(let i=0;i<100;i++){if(await Bun.file(join(scratch,"work/.tmp/paste-check")).size===bytes)break;await page.waitForTimeout(50)}
 assert(await Bun.file(join(scratch,"work/.tmp/paste-check")).text()===wrapped,"PTY received corrupt large paste");
 assert(sent.every(f=>new TextEncoder().encode(JSON.stringify(["stdin",f.data])).length<65536),"oversized stdin frame");
 assert(await page.locator(".webterm-terminal.-connected").count()===2,"large paste disconnected terminal");
 // IME must send the committed result, never intermediate composition text.
 await panes.nth(0).locator(".webterm-terminal").click();
 from=frames.length;
 await page.locator("[data-mobile-keyboard-input]").evaluate(input=>{
  input.dispatchEvent(new CompositionEvent("compositionstart",{bubbles:true}));
  input.dispatchEvent(new InputEvent("beforeinput",{inputType:"insertCompositionText",data:"ni",isComposing:true,bubbles:true}));
  (input as HTMLTextAreaElement).value="ni";
  input.dispatchEvent(new InputEvent("input",{inputType:"insertCompositionText",data:"ni",isComposing:true,bubbles:true}));
  input.dispatchEvent(new CompositionEvent("compositionend",{data:"你",bubbles:true}));
  input.dispatchEvent(new InputEvent("beforeinput",{inputType:"insertText",data:"你",bubbles:true,cancelable:true}));
 });
 await page.waitForTimeout(100);
 assert(frames.slice(from).map(f=>f.data).join("")==="你","IME committed text was lost or duplicated");
 await panes.nth(1).getByRole("button",{name:"Close pane",exact:true}).click();
 await page.getByRole("button",{name:"Close pane",exact:true}).last().click();
 await page.waitForFunction(()=>document.querySelectorAll(".pane").length===1);await helperCount();
 assert(await page.locator(".mobile-keybar").getAttribute("data-test-identity")==="original","closing a pane replaced the shared helper");
 await panes.nth(0).locator(".webterm-terminal").click();await typeMobile("SURVIVOR");
 assert(frames.some(f=>f.block===left&&f.data==="SURVIVOR"),"surviving pane lost keyboard input");
 // Closing the final pane disposes the document helper; a new tab recreates one.
 await page.getByRole("button",{name:"Close pane",exact:true}).first().click();
 await page.getByRole("button",{name:"Close pane",exact:true}).last().click();
 await page.waitForFunction(()=>document.querySelectorAll(".pane").length===0);await helperCount(0);
 await page.getByRole("button",{name:"New tab",exact:true}).first().click();
 await page.waitForSelector(".webterm-terminal.-connected");await helperCount();
 assert(!errors.length,"browser exceptions: "+errors.join("; "));
 // Desktop clipboard events use the renderer's input handler as well.
 await cdp.send("Emulation.setTouchEmulationEnabled",{enabled:false});
 await page.reload();await page.waitForSelector(".webterm-terminal.-connected");await helperCount(0);
 const desktop=page.locator(".webterm-terminal").first();await desktop.click();
 from=frames.length;
 await desktop.evaluate(input=>{
  const data=new DataTransfer();data.setData("text/plain","desktop-λ\ntext\x1b");
  input.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,bubbles:true,cancelable:true}));
 });
 await page.waitForTimeout(100);
 assert(frames.slice(from).map(f=>f.data).join("")==="desktop-λ\ntext ","desktop paste failed");
 // The renderer-owned desktop input uses the same native-copy contract.
 await desktop.click();await page.keyboard.press('Control+c');
 await page.keyboard.type("printf '\\033[2J\\033[HCOPY-%s\\n' DESKTOP");await page.keyboard.press('Enter');
 await page.waitForTimeout(200);
 const desktopCanvas=desktop.locator('canvas').first();const desktopBox=await desktopCanvas.boundingBox();
 const desktopCells=[...dimensions.values()].at(-1);assert(desktopBox&&desktopCells,'missing desktop metrics');
 await desktopCanvas.evaluate((canvas,point)=>{
  const clipboard=navigator.clipboard;const write=clipboard?.write,writeText=clipboard?.writeText;
  try{if(clipboard){clipboard.write=async()=>{};clipboard.writeText=async()=>{};}
   canvas.dispatchEvent(new MouseEvent('click',{detail:2,clientX:point.x,clientY:point.y,bubbles:true,cancelable:true}));
  }finally{if(clipboard){clipboard.write=write;clipboard.writeText=writeText;}}
 },{x:desktopBox.x+desktopBox.width/desktopCells.width*4.5,y:desktopBox.y+desktopBox.height/desktopCells.height*0.5});
 await desktop.dispatchEvent('contextmenu');
 const desktopInput=desktop.locator('textarea').first();assert(await desktopInput.inputValue()==='COPY-DESKTOP','desktop native copy did not stage selection');
 const desktopCopied=await desktopInput.evaluate(input=>{const data=new DataTransfer();input.dispatchEvent(new ClipboardEvent('copy',{clipboardData:data,bubbles:true,cancelable:true}));return data.getData('text/plain');});
 assert(desktopCopied==='COPY-DESKTOP'&&await desktopInput.inputValue()==='','desktop native copy payload failed');
 const exportResponse=page.waitForResponse(response=>response.url().includes('/api/capture?')&&response.request().method()==='GET');
 await page.getByRole('button',{name:'Command palette',exact:true}).click();
 await page.locator('#chooser-filter').fill('Export terminal as text');
 await page.getByRole('button',{name:'Export terminal as text',exact:true}).click();
 const exported=await exportResponse;
 assert(exported.ok()&&(await exported.text()).includes('COPY-DESKTOP'),'command-palette export did not read the focused server terminal');
 await checkReplay(page,`http://${address}`);
 await checkRenderer(page);
 assert(!errors.length,"browser exceptions: "+errors.join("; "));
 console.log("Browser smoke passed: mobile/desktop paste, bracketed paste, Unicode chunking, IME, focused-pane routing; one shared keyboard; no page exceptions.");
} finally {
 await page.close();
 if(!shared)await browser.close();
 server.kill();await server.exited;
 await rm(scratch,{recursive:true,force:true});
}
// Do not close the managed browser/context; disconnect this automation client.
if(shared)process.exit(0);
