// A disposable sandboxed service verifies that browser resources, mutations and
// terminal WebSockets stay below a reverse proxy's URL prefix.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {mkdtemp,rm} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {chromium,type Browser} from '../web/node_modules/playwright';
const scratch=await mkdtemp('/tmp/rexweb-prefix-');
let child:ReturnType<typeof spawn>|undefined,browser:Browser|undefined,logs='';
try{
 child=spawn(resolve('bin/rexweb'),['-mode','web','-listen','127.0.0.1:0','-base-path','/terminal/','-workspace',join(scratch,'work'),'-session-socket',join(scratch,'run/session.sock')],{stdio:['ignore','ignore','pipe']});
 child.stderr!.on('data',data=>logs+=data);
 for(let i=0;i<200&&!logs.includes('Ready:');i++){assert.equal(child.exitCode,null,logs);await Bun.sleep(50);}
 const address=logs.match(/Web listener at (\S+)/)?.[1];assert(address,logs);const origin='http://'+address;
 assert.equal((await fetch(origin+'/api/sessions')).status,404);
 browser=await chromium.launch({executablePath:process.env.CHROMIUM_PATH,args:['--no-sandbox','--no-proxy-server']});
 const page=await browser.newPage();const errors:string[]=[],requests:string[]=[],sockets:string[]=[];
 page.on('pageerror',e=>errors.push(String(e)));
 page.on('request',r=>requests.push(new URL(r.url()).pathname));
 page.on('websocket',ws=>sockets.push(new URL(ws.url()).pathname));
 await page.goto(origin+'/terminal/');await page.locator('.webterm-terminal.-connected').waitFor();
 assert.equal(await page.locator('base').getAttribute('href'),'/terminal/');
 await page.getByRole('button',{name:'New tab',exact:true}).click();
 await page.waitForFunction(()=>document.querySelectorAll('.window-tab').length===2);
 await page.locator('.webterm-terminal.-connected').waitFor();
 await page.evaluate(()=>document.fonts.ready);
 assert(requests.includes('/terminal/static/js/ghostty-vt.wasm'),'WASM did not load below prefix');
 assert(requests.includes('/terminal/api/action'),'mutation did not use prefix');
 assert(sockets.length>0&&sockets.every(path=>path==='/terminal/api/terminal'),'WebSocket escaped prefix');
 assert(!requests.some(path=>path.startsWith('/api/')||path.startsWith('/static/')),'resource escaped prefix');
 assert.deepEqual(errors,[]);
 console.log(JSON.stringify({result:'passed',checks:['sandboxed service','prefixed CSS/JS/WASM/API','prefixed mutations and WebSockets','root API unavailable'],browserErrors:errors.length}));
}finally{
 if(browser)await browser.close();if(child&&child.exitCode===null){child.kill();await once(child,'exit');}
 await rm(scratch,{recursive:true,force:true});
}
