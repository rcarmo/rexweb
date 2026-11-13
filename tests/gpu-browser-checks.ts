import type { Page } from '../web/node_modules/playwright';
import { resolve, join } from 'node:path';

/** Public SDK GPU presentation/device-loss regression (not production test hooks). */
export async function checkGPU(page: Page, requireGPU = false) {
  const root = resolve(process.env.GHOSTTY_BROWSER_ROOT || 'web/node_modules/ghostty-web');
  const host = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === '/') return new Response(`<!doctype html><div id="host"></div><script type="module">
      import { Ghostty, Terminal } from '/ghostty-web.js';
      const ghostty = await Ghostty.load('/ghostty-vt.wasm');
      window.term = new Terminal({ ghostty, renderer: 'webgpu', cols: 20, rows: 4, cursorBlink: false, theme: { background: '#ff0000' } });
      await window.term.openAsync(document.querySelector('#host'));
      window.term.write('GPU-PRESERVED');
      window.ready = true;
    </script>`, { headers: { 'Content-Type': 'text/html' } });
    if (/^\/[a-zA-Z0-9_.-]+\.js$/.test(path)) return new Response(Bun.file(join(root, 'dist', path.slice(1))));
    if (path === '/ghostty-vt.wasm') return new Response(Bun.file(join(root, 'ghostty-vt.wasm')));
    return new Response('not found', { status: 404 });
  }});
  const assert = (value: unknown, message: string) => { if (!value) throw new Error(message); };
  try {
    await page.goto(`http://127.0.0.1:${host.port}`);
    await page.waitForFunction(() => (window as any).ready);
    const backend = await page.evaluate(() => (window as any).term.renderer.backend ?? 'canvas');
    assert(!requireGPU || backend === 'webgpu', 'GPU path required but unavailable');
    if (backend === 'webgpu') {
      await page.evaluate(() => {
        const term = (window as any).term;
        // Green CSS fallback cannot satisfy the red GPU pixel assertion.
        term.renderer.getCanvas().style.backgroundColor = '#00ff00';
        term.refresh();
      });
      const image = await page.evaluate(async () => {
        const r = (window as any).term.renderer;
        // Read the actual WGSL output, not the CPU raster or CSS background.
        // Copy in the same task: WebGPU releases its canvas texture at presentation.
        r.present();
        const gpu = (window.navigator as any).gpu;
        const destination = r.device.createBuffer({ size:256, usage:9 }); // MAP_READ | COPY_DST
        const encoder = r.device.createCommandEncoder();
        encoder.copyTextureToBuffer({ texture:r.context.getCurrentTexture(), origin:[20,20] }, { buffer:destination, bytesPerRow:256 }, [1,1]);
        r.device.queue.submit([encoder.finish()]);
        await destination.mapAsync(1); // GPUMapMode.READ
        const pixel = [...new Uint8Array(destination.getMappedRange()).slice(0,4)];
        destination.unmap(); destination.destroy();
        if (gpu.getPreferredCanvasFormat() === 'bgra8unorm') [pixel[0],pixel[2]] = [pixel[2],pixel[0]];
        return pixel;
      });
      assert(image[0] > 240 && image[1] < 10 && image[2] < 10, 'WGSL presentation did not produce red pixels: ' + image);
      await page.evaluate(() => {
        const term = (window as any).term;
        term.select(0, 0, 13);
        (window as any).selection = term.getSelection();
        (window as any).oldCanvas = term.renderer.getCanvas();
        term.renderer.device.destroy();
      });
      await page.waitForFunction(() => (window as any).term.renderer.backend === 'canvas');
      assert(await page.evaluate(() => {
        const term = (window as any).term;
        return term.getSelection() === (window as any).selection && term.renderer.getCanvas() !== (window as any).oldCanvas && term.renderer.getCanvas().parentElement?.id === 'host';
      }), 'device-loss fallback lost selection or canvas bindings');
      await page.evaluate(() => (window as any).term.write('\r\nAFTER-LOSS'));
      assert(await page.evaluate(() => (window as any).term.buffer.active.getLine(1).translateToString().includes('AFTER-LOSS')), 'VT did not survive GPU loss');
    }
    console.log('GPU browser checks:', backend === 'webgpu' ? 'WGSL pixels + actual device-loss fallback passed' : 'Canvas fallback passed (GPU adapter unavailable)');
  } finally { await page.evaluate(() => (window as any).term?.dispose()).catch(() => {}); host.stop(true); }
}

if (import.meta.main) {
  const { chromium } = await import('../web/node_modules/playwright');
  const browser = await chromium.launch({ executablePath: '/opt/kintsugi/bin/chromium', args: ['--enable-unsafe-webgpu', '--use-angle=swiftshader', '--use-vulkan=swiftshader', '--enable-features=Vulkan', '--disable-vulkan-surface'] });
  try {
    const page = await browser.newPage();
    page.on('pageerror', error => console.error(error));
    page.on('console', msg => { if (msg.type() === 'error' || msg.type() === 'warning') console.log(msg.text()); });
    await checkGPU(page, process.env.GPU_REQUIRED === '1');
  }
  finally { await browser.close(); }
}
