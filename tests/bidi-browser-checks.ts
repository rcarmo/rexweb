import type { Page } from '../web/node_modules/playwright';
import { resolve, join } from 'node:path';

export async function checkBidi(page: Page) {
  const root = resolve(process.env.GHOSTTY_BROWSER_ROOT || 'web/node_modules/ghostty-web');
  const host = Bun.serve({ hostname:'127.0.0.1',port:0,fetch(request) {
    const path=new URL(request.url).pathname;
    if(path==='/') return new Response(`<!doctype html><div id="host"></div><script type="module">
      import { Terminal, Ghostty } from '/ghostty-web.js';
      const ghostty=await Ghostty.load('/ghostty-vt.wasm');
      window.term=new Terminal({ghostty,bidi:true,cols:20,rows:4,cursorBlink:false,copyOnSelect:false,onLinkClick:url=>{window.clicked=url;return true;}});
      window.term.open(document.querySelector('#host'));
      window.term.write('אבגד');window.ready=true;
    </script>`,{headers:{'Content-Type':'text/html; charset=utf-8'}});
    if(/^\/[a-zA-Z0-9_.-]+\.js$/.test(path))return new Response(Bun.file(join(root,'dist',path.slice(1))));
    if(path==='/ghostty-vt.wasm')return new Response(Bun.file(join(root,'ghostty-vt.wasm')));
    return new Response('not found',{status:404});
  }});
  const assert=(value:unknown,message:string)=>{if(!value)throw new Error(message)};
  try {
    await page.goto(`http://127.0.0.1:${host.port}`);
    await page.waitForFunction(()=>(window as any).ready);
    const painted=await page.evaluate(async()=>{
      const term=(window as any).term,r=term.renderer,original=r.ctx.fillText,characters:string[]=[];
      r.ctx.fillText=function(text:string,...args:any[]){characters.push(text);return original.call(this,text,...args)};
      term.refresh();await new Promise<void>(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve())));
      r.ctx.fillText=original;return characters.slice(0,4).join('');
    });
    assert(painted==='דגבא','RTL visual paint order mismatch: '+painted);
    const canvas=page.locator('#host > canvas').first(),box=await canvas.boundingBox();
    const metrics=await page.evaluate(()=>(window as any).term.renderer.getMetrics());
    assert(box,'missing RTL canvas');
    const x=(column:number)=>box!.x+(column+.5)*metrics.width,y=box!.y+metrics.height*.5;
    await page.mouse.move(x(3),y);await page.mouse.down();await page.mouse.move(x(2),y);await page.mouse.up();
    assert(await page.evaluate(()=>(window as any).term.getSelection())==='אב','RTL linear gesture copied visual rather than logical text');
    await page.keyboard.down('Alt');await page.mouse.move(x(0),y);await page.mouse.down();await page.mouse.move(x(1),y);await page.mouse.up();await page.keyboard.up('Alt');
    assert(await page.evaluate(()=>(window as any).term.getSelection())==='גד','RTL rectangle copied the wrong logical cells');
    await page.evaluate(()=>{
      const term=(window as any).term;term.reset();term.write('\x1b]8;;https://example.com/rtl\x1b\\א\x1b]8;;\x1b\\בגד\x1b[1;1H');
    });
    await canvas.click({position:{x:metrics.width*3.5,y:metrics.height*.5}});
    await page.waitForFunction(()=>(window as any).clicked==='https://example.com/rtl');
    assert(await page.evaluate(()=>(window as any).term.getVisualCursor().x)===3,'RTL cursor/IME anchor mismatch');
    await page.evaluate(()=>{const term=(window as any).term;(window as any).reports=[];term.onData((data:string)=>(window as any).reports.push(data));term.write('\x1b[?1000h\x1b[?1006h')});
    await canvas.click({position:{x:metrics.width*.5,y:metrics.height*.5}});
    assert(await page.evaluate(()=>(window as any).reports.some((data:string)=>data.includes('\x1b[<0;4;1M'))),'RTL SGR mouse report did not use logical columns');
    console.log('BiDi browser checks: visual painting, linear/rectangle copy, OSC8 links, cursor and SGR mouse mapping passed');
  } finally {await page.evaluate(()=>(window as any).term?.dispose()).catch(()=>{});host.stop(true)}
}

if(import.meta.main){
  const { chromium }=await import('../web/node_modules/playwright');
  const browser=await chromium.launch({executablePath:'/opt/kintsugi/bin/chromium'});
  try {await checkBidi(await browser.newPage())} finally {await browser.close()}
}
