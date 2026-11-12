import type { Page } from "../web/node_modules/playwright";
import { join, resolve } from "node:path";

// Exercise the installed public renderer API separately from the workspace UI,
// without exposing test hooks in the production browser bundle.
export async function checkRenderer(page: Page) {
  const base = resolve("web/node_modules/ghostty-web");
  const assets = new Map<string, ReturnType<typeof Bun.file>>();
  assets.set("/ghostty-vt.wasm", Bun.file(join(base, "ghostty-vt.wasm")));
  for await (const name of new Bun.Glob("*.js").scan({ cwd: join(base, "dist") })) {
    assets.set("/" + name, Bun.file(join(base, "dist", name)));
  }
  const host = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch(request) {
      const path = new URL(request.url).pathname;
      if (path === "/") return new Response(`<!doctype html>
        <div id="host" style="width:640px;height:340px"></div>
        <script type="module">
          import { Ghostty, Terminal } from '/ghostty-web.js';
          const ghostty = await Ghostty.load('/ghostty-vt.wasm');
          window.rendererTerm = new Terminal({ ghostty, cols: 40, rows: 8,
            fontSize: 14, fontFamily: 'monospace', cursorBlink: false,
            theme: { background: '#112233', foreground: '#ddeeff' },
            onLinkClick: url => { window.clickedURL = url; return true; } });
          window.rendererTerm.open(document.querySelector('#host'));
          window.rendererTerm.write('BASELINE');
          await document.fonts.ready;
          window.rendererReady = true;
        </script>`, { headers: { "Content-Type": "text/html" } });
      const asset = assets.get(path);
      return asset ? new Response(asset) : new Response("not found", { status: 404 });
    },
  });
  const assert = (condition: unknown, message: string) => { if (!condition) throw new Error(message); };
  const settle = () => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  const bitmap = () => page.evaluate(() => (window as any).rendererTerm.renderer.getCanvas().toDataURL());
  const write = (text: string) => page.evaluate(text => (window as any).rendererTerm.write(text), text);
  const clearBitmap = () => page.evaluate(() => {
    const canvas = (window as any).rendererTerm.renderer.getCanvas() as HTMLCanvasElement;
    canvas.getContext("2d")!.clearRect(0, 0, canvas.width, canvas.height);
  });
  try {
    await page.goto(`http://127.0.0.1:${host.port}`);
    await page.waitForFunction(() => (window as any).rendererReady);
    await settle();
    let before = await bitmap();
    await write("\x1b[?2026h\x1b[2J\x1b[HHELD FRAME");
    await settle();
    assert(await bitmap() === before, "DEC 2026 painted a partial frame");
    await page.evaluate(() => (window as any).rendererTerm.refresh());
    await settle();
    assert(await bitmap() === before, "refresh bypassed the held frame");
    await write("\x1b[?2026l");
    await settle();
    assert(await bitmap() !== before, "DEC 2026 end did not present the completed frame");

    before = await bitmap();
    await page.evaluate(() => {
      const term = (window as any).rendererTerm;
      term.input("x", true);
      term.write("\x1b[?2026h\x1b[2J\x1b[HECHO FRAME");
    });
    await settle();
    assert(await bitmap() === before, "synchronous echo bypassed DEC 2026");
    await write("\x1b[?2026l");
    await settle();
    assert(await bitmap() !== before, "held echo never appeared");

    before = await bitmap();
    await write("\x1b[?2026h\x1b[2J\x1b[HDEADLINE FRAME");
    await settle();
    assert(await bitmap() === before, "missing-end transaction painted too early");
    await page.waitForTimeout(1150);
    await settle();
    assert(await bitmap() !== before, "missing DEC end marker froze the canvas");
    await write("\x1b[?2026l");
    await settle();

    const restored = await bitmap();
    for (const event of ["pageshow", "contextrestored", "webglcontextrestored"]) {
      await clearBitmap();
      assert(await bitmap() !== restored, "test failed to clear canvas backing pixels");
      await page.evaluate(event => {
        if (event === "pageshow") window.dispatchEvent(new PageTransitionEvent("pageshow"));
        else (window as any).rendererTerm.renderer.getCanvas().dispatchEvent(new Event(event));
      }, event);
      await settle();
      assert(await bitmap() === restored, event + " failed to recover the canvas");
    }
    await page.evaluate(() => (window as any).rendererTerm.suspend());
    await clearBitmap();
    await page.evaluate(() => (window as any).rendererTerm.resume());
    await settle();
    assert(await bitmap() === restored, "resume failed to repaint a clean terminal");

    const wheel = await page.evaluate(() => {
      const term = (window as any).rendererTerm;
      const sent: string[] = [];
      const listener = term.onData((text: string) => sent.push(text));
      term.write("\x1b[?1049h\x1b[?1000h\x1b[?1006h");
      const canvas = term.renderer.getCanvas();
      const rect = canvas.getBoundingClientRect();
      canvas.dispatchEvent(new WheelEvent("wheel", { deltaY: -100,
        clientX: rect.left + term.renderer.charWidth * 2.5,
        clientY: rect.top + term.renderer.charHeight * 1.5,
        bubbles: true, cancelable: true }));
      listener.dispose();
      term.write("\x1b[?1000l\x1b[?1049l");
      return sent;
    });
    assert(JSON.stringify(wheel) === JSON.stringify(["\x1b[<64;3;2M"]), "browser wheel routing duplicated or lost the SGR report");

    const url = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
    await write("\x1b[2J\x1b[H" + url);
    await settle();
    await page.evaluate(() => {
      const term = (window as any).rendererTerm;
      const canvas = term.renderer.getCanvas();
      const rect = canvas.getBoundingClientRect();
      canvas.dispatchEvent(new MouseEvent("click", { ctrlKey: true,
        clientX: rect.left + term.renderer.charWidth * 3.5,
        clientY: rect.top + term.renderer.charHeight * 0.5,
        bubbles: true, cancelable: true }));
    });
    await page.waitForFunction(url => (window as any).clickedURL === url, url);
    await page.evaluate(()=>{
      const term=(window as any).rendererTerm;term.reset();term.write('ABCDE\r\nFGHIJ\r\nKLMNO');term.selectRectangle(1,0,3,2);
      if(term.getSelection()!=='BCD\nGHI\nLMN')throw new Error('core rectangular copy failed');
    });
    const cdp=await page.context().newCDPSession(page);
    for(const ratio of [1,1.25,1.5,2]){
      await cdp.send('Emulation.setDeviceMetricsOverride',{width:800,height:900,deviceScaleFactor:ratio,mobile:false});
      await page.evaluate(()=>window.dispatchEvent(new Event('resize')));await settle();
      const grid=await page.evaluate(()=>{const term=(window as any).rendererTerm,canvas=term.renderer.getCanvas(),metrics=term.renderer.getMetrics();return {width:canvas.width,height:canvas.height,cols:term.cols,rows:term.rows,pitch:metrics.width*devicePixelRatio,line:metrics.height*devicePixelRatio};});
      assert(Math.abs(grid.pitch-Math.round(grid.pitch))<1e-8&&Math.abs(grid.line-Math.round(grid.line))<1e-8,'fractional physical grid');
      assert(grid.width===grid.cols*Math.round(grid.pitch)&&grid.height===grid.rows*Math.round(grid.line),'backing dimensions disagree with physical grid');
    }
    await cdp.send('Emulation.clearDeviceMetricsOverride');
    await page.evaluate(()=>{
      const term=(window as any).rendererTerm;term.reset();term.write('\x1b[?25l\x1b_Ga=T,f=32,s=1,v=1,i=1,c=2,r=2,C=1,z=1;/wAA/w==\x1b\\');
    });
    await page.waitForFunction(()=>{const term=(window as any).rendererTerm;const pixel=term.renderer.getCanvas().getContext('2d').getImageData(3,3,1,1).data;return pixel[0]===255&&pixel[2]===0;});
    await page.evaluate(()=>{const term=(window as any).rendererTerm;term.write('\x1b_Ga=f,f=32,i=1,s=1,v=1,z=100;AAD//w==\x1b\\\x1b_Ga=a,i=1,r=1,z=100\x1b\\\x1b_Ga=a,i=1,s=3\x1b\\');});
    await page.waitForFunction(()=>{const term=(window as any).rendererTerm;const pixel=term.renderer.getCanvas().getContext('2d').getImageData(3,3,1,1).data;return pixel[2]===255&&pixel[0]===0;});
    await page.evaluate(() => (window as any).rendererTerm.dispose());
    console.log("Renderer browser checks passed: held frames, echo guard, timeout, canvas recovery, SGR wheel and balanced-link activation.");
  } finally { await host.stop(true); }
}
