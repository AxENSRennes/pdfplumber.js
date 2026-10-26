// A browser application that bundles the package (scripts/check-package.mjs
// --bundles builds it with esbuild and with Vite, following the README's
// recipes; js/test/browser/bundlers.spec.ts runs it in Chromium). The page
// sets `window.SETTINGS`, the configure() of its recipe.
import { configure, open } from "pdfplumber-js";

window.quickStart = async ({ worker, url }) => {
  const settings = window.SETTINGS ?? {};
  configure({ ...settings, worker: worker ? (settings.worker ?? true) : false });
  const pdf = await open(url);
  try {
    const [page] = await pdf.pages;
    return { text: await page.extractText(), words: (await page.extractWords()).length };
  } finally {
    await pdf.close();
    await pdf.release();
  }
};
window.ready = true;
