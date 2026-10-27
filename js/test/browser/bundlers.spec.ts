// The package bundled into an application, with esbuild and with Vite,
// following the README's recipes ("Bundlers and CDN"; built from
// the packed package by scripts/check-package.mjs --bundles into
// dist/bundler-tests/), in the page and in worker mode, on a Latin and a
// CJK document (its CMaps fetched from where the recipe put them). The
// results are those of the Node build of this checkout (itself checked
// against Python by the sessions of the API). Chromium only: the recipes
// are about the bundlers, not the engines.
import { existsSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

import * as api from "../../../dist/src/index.js";
import { PDFPLUMBER_ROOT, ROOT } from "../oracle-data.mjs";

const FILES = ["corpus/micro/text-operators.pdf", "upstream/pdfplumber-python/tests/pdfs/pr-136-example.pdf"];

async function expected(rel: string): Promise<{ text: string; words: number }> {
  const pdf = await api.open(join(PDFPLUMBER_ROOT, rel));
  try {
    const [page] = await pdf.pages;
    return { text: await page.extractText(), words: (await page.extractWords()).length };
  } finally {
    await pdf.release();
  }
}

for (const bundler of ["esbuild", "vite"]) {
  test.describe(`bundled with ${bundler}`, () => {
    test.skip(({ browserName }) => browserName !== "chromium", "the recipes are checked in Chromium");
    test.skip(!existsSync(join(ROOT, "dist/bundler-tests", bundler, "index.html")), "no bundle: node scripts/check-package.mjs --bundles");
    for (const worker of [false, true]) {
      for (const rel of FILES) {
        test(`${worker ? "worker" : "page"}: ${rel}`, async ({ page }) => {
          const errors: string[] = [];
          page.on("pageerror", (e) => errors.push(e.message));
          await page.goto(`/dist/bundler-tests/${bundler}/index.html`);
          await page.waitForFunction(() => (window as unknown as { ready?: boolean }).ready);
          const got = await page.evaluate(
            ([w, url]) =>
              (window as unknown as { quickStart(o: object): Promise<{ text: string; words: number }> }).quickStart({ worker: w, url }),
            [worker, `/${rel}`] as const,
          );
          expect(got).toEqual(await expected(rel));
          expect(errors).toEqual([]);
        });
      }
    }
  });
}
