#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import * as mupdf from "mupdf";

const DEFAULT_PDFS = [
  "test/fixtures/external-pdfs/pdfjs-identity-to-unicode-map.pdf",
  "test/fixtures/external-pdfs/pdfjs-truetype-without-cmap.pdf",
  "test/fixtures/external-pdfs/pdfjs-type3-word-spacing.pdf",
  "test/fixtures/external-pdfs/pdfjs-arabic-cidtruetype.pdf",
  "test/fixtures/external-pdfs/verapdf-pdfua-table-pass.pdf",
  "test/fixtures/external-pdfs/pdfjs-bitmap-mmr.pdf",
  "test/fixtures/external-pdfs/pdfjs-annotation-text-widget.pdf",
  "test/fixtures/external-holdout-pdfs/pdfjs-issue6068.pdf",
  "test/fixtures/external-holdout-pdfs/cms-1500.pdf",
  "test/fixtures/external-holdout-pdfs/boj-semiannual-2024-jp.pdf"
];

function argValue(name, fallback = null) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] ?? fallback : fallback;
}

function round(value) {
  return Number.isFinite(value) ? Math.round(value * 1000) / 1000 : null;
}

function textPrefix(value, limit = 80) {
  return value.replace(/\s+/g, " ").trim().slice(0, limit);
}

function normalizeText(value) {
  return value.replace(/\s+/g, " ").trim();
}

function rectFromQuad(quad) {
  const xs = [quad[0], quad[2], quad[4], quad[6]];
  const ys = [quad[1], quad[3], quad[5], quad[7]];
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

function average(values) {
  return values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null;
}

function levenshteinRatio(a, b) {
  if (a === b) return 1;
  if (!a.length || !b.length) return 0;
  const prev = Array.from({ length: b.length + 1 }, (_, i) => i);
  const curr = new Array(b.length + 1);
  for (let i = 1; i <= a.length; i += 1) {
    curr[0] = i;
    for (let j = 1; j <= b.length; j += 1) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      curr[j] = Math.min(prev[j] + 1, curr[j - 1] + 1, prev[j - 1] + cost);
    }
    for (let j = 0; j <= b.length; j += 1) prev[j] = curr[j];
  }
  return 1 - prev[b.length] / Math.max(a.length, b.length);
}

function collectMupdfPage(page) {
  const chars = [];
  let imageCount = 0;
  let vectorCount = 0;
  const structuredText = page.toStructuredText("preserve-whitespace,preserve-spans,preserve-images");
  const text = structuredText.asText();
  structuredText.walk({
    onChar(c, origin, font, size, quad, color) {
      chars.push({ text: c, origin, fontname: font.getName(), size, bbox: rectFromQuad(quad), color });
    },
    onImageBlock() {
      imageCount += 1;
    },
    onVector() {
      vectorCount += 1;
    }
  });
  return { chars, text, imageCount, vectorCount };
}

async function comparePdf(open, pdfPath, pageLimit, password) {
  const bytes = readFileSync(pdfPath);
  const mupdfDoc = mupdf.Document.openDocument(bytes, "application/pdf");
  const needsPassword = mupdfDoc.needsPassword?.() ?? false;
  if (needsPassword) {
    if (!password) throw new Error("No password given");
    const auth = mupdfDoc.authenticatePassword(password);
    if (!auth) throw new Error("Invalid password");
  }
  const portDoc = await open(pdfPath, needsPassword ? { password } : {});
  const mupdfPageCount = mupdfDoc.countPages();
  const portPageCount = portDoc.pages.length;
  const pages = Math.min(mupdfPageCount, portPageCount, pageLimit);
  const pageSummaries = [];

  for (let index = 0; index < pages; index += 1) {
    const mupdfPage = mupdfDoc.loadPage(index);
    const mupdfSummary = collectMupdfPage(mupdfPage);
    const portPage = portDoc.pages[index];
    const portText = await portPage.extractText();
    const mupdfLinks = mupdfPage.getLinks?.().length ?? 0;
    const mupdfAnnots = mupdfPage.getAnnotations?.().length ?? 0;
    const mupdfWidgets = mupdfPage.getWidgets?.().length ?? 0;
    const comparable = Math.min(mupdfSummary.chars.length, portPage.chars.length, 500);
    const dx = [];
    const topDy = [];
    const bottomDy = [];
    const widthDelta = [];
    const textMismatches = [];

    for (let i = 0; i < comparable; i += 1) {
      const mu = mupdfSummary.chars[i];
      const current = portPage.chars[i];
      if (mu.text !== current.text && textMismatches.length < 5) textMismatches.push({ index: i, mupdf: mu.text, port: current.text });
      dx.push(Math.abs(mu.bbox[0] - Number(current.x0 ?? 0)));
      topDy.push(Math.abs(mu.bbox[1] - Number(current.top ?? 0)));
      bottomDy.push(Math.abs(mu.bbox[3] - Number(current.bottom ?? 0)));
      widthDelta.push(Math.abs((mu.bbox[2] - mu.bbox[0]) - Number(current.width ?? 0)));
    }

    pageSummaries.push({
      page: index + 1,
      chars: {
        mupdf: mupdfSummary.chars.length,
        port: portPage.chars.length,
        delta: portPage.chars.length - mupdfSummary.chars.length,
        firstMismatches: textMismatches
      },
      text: {
        similarity: round(levenshteinRatio(mupdfSummary.text, portText)),
        normalizedSimilarity: round(levenshteinRatio(normalizeText(mupdfSummary.text), normalizeText(portText))),
        mupdfPrefix: textPrefix(mupdfSummary.text),
        portPrefix: textPrefix(portText)
      },
      geometry: {
        comparedChars: comparable,
        avgAbsX0Delta: round(average(dx)),
        avgAbsTopDelta: round(average(topDy)),
        avgAbsBottomDelta: round(average(bottomDy)),
        avgAbsWidthDelta: round(average(widthDelta))
      },
      objects: {
        mupdfImages: mupdfSummary.imageCount,
        mupdfVectorsFromStructuredText: mupdfSummary.vectorCount,
        mupdfLinks,
        mupdfAnnots,
        mupdfWidgets,
        portImages: portPage.images.length,
        portLines: portPage.lines.length,
        portRects: portPage.rects.length,
        portCurves: portPage.curves.length,
        portAnnots: portPage.annots.length
      }
    });
    mupdfPage.destroy();
  }

  await portDoc.close();
  mupdfDoc.destroy();
  return {
    pdf: pdfPath,
    pagesCompared: pages,
    pagesAvailable: { mupdf: mupdfPageCount, port: portPageCount },
    pages: pageSummaries
  };
}

const buildIndex = path.resolve("dist/src/index.js");
if (!existsSync(buildIndex)) {
  console.error("dist/src/index.js is missing. Run `npm run build` first.");
  process.exit(1);
}

const { open } = await import(pathToFileURL(buildIndex).href);
const pageLimit = Number(argValue("--pages", "2"));
const password = argValue("--password", null);
const positional = process.argv.slice(2).filter((arg, index, args) => {
  if (arg.startsWith("--")) return false;
  return !args[index - 1]?.startsWith("--");
});
const pdfs = (positional.length ? positional : DEFAULT_PDFS).filter((pdf) => existsSync(pdf));

const results = [];
for (const pdf of pdfs) {
  try {
    results.push(await comparePdf(open, pdf, pageLimit, password));
  } catch (error) {
    results.push({ pdf, error: error instanceof Error ? error.message : String(error) });
  }
}

console.log(JSON.stringify({ generatedAt: new Date().toISOString(), pageLimit, results }, null, 2));
