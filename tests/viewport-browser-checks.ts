import { chromium, firefox, webkit, type Page } from '../web/node_modules/playwright';
import { mkdtemp, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';

/** Synthetic Safari viewport events; not a claim of physical iPad validation. */
export async function checkViewport(page: Page) {
  const assert = (value: unknown, message: string) => { if (!value) throw new Error(message); };
  await page.locator('.webterm-terminal').first().click();
  await page.evaluate(() => {
    const viewport = window.visualViewport!;
    if (!viewport) throw new Error('VisualViewport unavailable');
    const properties = ['offsetLeft', 'offsetTop', 'width', 'height', 'scale'] as const;
    const originals = Object.fromEntries(properties.map(key => [key, Object.getOwnPropertyDescriptor(viewport, key)]));
    const input = document.querySelector('[data-mobile-keyboard-input]');
    if (!input) throw new Error('Shared mobile input missing');
    (window as any).__viewportCheck = { viewport, originals, input };
  });
  try {
    for (const metrics of [
      { offsetLeft:0, offsetTop:64, width:800, height:700, scale:1 }, // accessory/indicator pan
      { offsetLeft:0, offsetTop:120, width:800, height:700, scale:1 }, // pan without any resize
      { offsetLeft:0, offsetTop:0, width:800, height:420, scale:1 }, // docked keyboard
      { offsetLeft:0, offsetTop:120, width:800, height:520, scale:1 },
    ]) {
      await page.evaluate(metrics => {
        const viewport = (window as any).__viewportCheck.viewport;
        for (const [key, value] of Object.entries(metrics)) Object.defineProperty(viewport, key, { configurable:true, value });
        viewport.dispatchEvent(new Event('resize'));
        viewport.dispatchEvent(new Event('scroll'));
      }, metrics);
      await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
      const geometry = await page.evaluate(() => {
        const input = document.querySelector('[data-mobile-keyboard-input]') as HTMLTextAreaElement;
        const body = document.body.getBoundingClientRect();
        const caret = input.getBoundingClientRect();
        const canvas = document.querySelector('.webterm-terminal canvas')!.getBoundingClientRect();
        return {
          bodyTop:body.top, bodyHeight:body.height,
          caretTop:caret.top, caretBottom:caret.bottom, canvasTop:canvas.top, canvasBottom:canvas.bottom,
          fixed:getComputedStyle(document.body).position,
          inputs:document.querySelectorAll('[data-mobile-keyboard-input]').length,
          same:input === (window as any).__viewportCheck.input,
          focused:document.activeElement === input,
          wrap:input.wrap, padding:getComputedStyle(input).paddingTop,
        };
      });
      assert(geometry.fixed === 'fixed' && Math.abs(geometry.bodyTop-metrics.offsetTop) < 1, 'workspace top does not follow visual viewport pan');
      assert(Math.abs(geometry.bodyHeight-metrics.height) < 1, 'workspace does not fit visible keyboard viewport');
      assert(geometry.caretTop >= metrics.offsetTop && geometry.caretBottom <= metrics.offsetTop+metrics.height, 'native caret extends outside visible viewport');
      assert(geometry.caretTop >= geometry.canvasTop && geometry.caretBottom <= geometry.canvasBottom, 'viewport pan detached the shared input from its pane');
      assert(geometry.inputs === 1 && geometry.same && geometry.focused, 'viewport changes replaced or blurred the shared input');
      assert(geometry.wrap === 'off' && geometry.padding === '0px', 'native input can create an overflowing caret box');
    }
    await page.evaluate(() => {
      const viewport = (window as any).__viewportCheck.viewport;
      Object.defineProperty(viewport, 'scale', { configurable:true, value:2 });
      viewport.dispatchEvent(new Event('resize'));
    });
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    assert(await page.evaluate(() => document.body.getBoundingClientRect().top === 0 && Math.abs(document.body.getBoundingClientRect().height-window.innerHeight) < 1), 'pinch zoom was treated as a keyboard resize');
    console.log('Viewport browser checks passed: accessory pan, docked keyboard, full caret bounds, shared focus and pinch-zoom policy. Physical iPad validation remains required.');
  } finally {
    await page.evaluate(() => {
      const state = (window as any).__viewportCheck;
      for (const [key, descriptor] of Object.entries(state.originals)) {
        if (descriptor) Object.defineProperty(state.viewport, key, descriptor as PropertyDescriptor);
        else delete state.viewport[key];
      }
      state.viewport.dispatchEvent(new Event('resize'));
      state.viewport.dispatchEvent(new Event('scroll'));
      delete (window as any).__viewportCheck;
    });
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  }
}

if (import.meta.main) {
  const scratch = await mkdtemp('/tmp/rexweb-viewport-browser.');
  const server = Bun.spawn([resolve('bin/rexweb'), '-mode','web','-listen','127.0.0.1:0','-session-socket',join(scratch,'run/session.sock'),'-workspace',join(scratch,'work')], { stdout:'pipe', stderr:'pipe' });
  let logs=''; void(async()=>{for await(const chunk of server.stderr) logs+=new TextDecoder().decode(chunk)})();
  let browser;
  try {
    for(let i=0;i<100&&!logs.includes('Ready:');i++){if(server.exitCode!==null)throw new Error(logs);await Bun.sleep(50)}
    const address=logs.match(/Web listener at (\S+)/)?.[1];if(!address)throw new Error('Viewport fixture not ready: '+logs);
    const type = ({ chromium, firefox, webkit } as const)[process.env.VIEWPORT_BROWSER as 'chromium'|'firefox'|'webkit' || 'chromium'];
    if(!type)throw new Error('Unknown VIEWPORT_BROWSER');
    browser=await type.launch();
    const page=await browser.newPage({hasTouch:true,viewport:{width:800,height:900},userAgent:'Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1'});
    const errors:string[]=[];page.on('pageerror',error=>errors.push(String(error)));
    await page.goto('http://'+address);await page.waitForSelector('.webterm-terminal.-connected');
    await checkViewport(page);
    if(errors.length)throw new Error(errors.join('\n'));
  } finally {await browser?.close();server.kill();await server.exited;await rm(scratch,{recursive:true,force:true})}
}
