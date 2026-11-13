import { firefox, webkit } from '../web/node_modules/playwright';
import { checkRenderer } from './renderer-browser-checks';
import { checkBidi } from './bidi-browser-checks';
import { checkGPU } from './gpu-browser-checks';

// Requires Playwright Firefox/WebKit and host libraries. No GPU display assertion.
for (const [name, type] of Object.entries({ firefox, webkit })) {
  const browser = await type.launch();
  try {
    const page = await browser.newPage();
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(String(error)));
    await checkRenderer(page);
    await checkBidi(page);
    await checkGPU(page);
    if (errors.length) throw new Error(errors.join('\n'));
    console.log(`${name}: Canvas, images, BiDi and available-backend/fallback checks passed`);
  } finally { await browser.close(); }
}
