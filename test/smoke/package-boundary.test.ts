import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function read(relativePath: string): string {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function flatText(text: string): string {
  return text.replace(/\s+/g, " ");
}

function walk(relativePath: string): string[] {
  const absolutePath = path.join(repoRoot, relativePath);
  return fs.readdirSync(absolutePath, { withFileTypes: true }).flatMap((entry) => {
    const child = path.join(relativePath, entry.name);
    return entry.isDirectory() ? walk(child) : [child];
  });
}

describe("package boundary", () => {
  it("documents the stable public extraction contract", () => {
    const readme = read("README.md");
    const publicApi = read("docs/public-api.md");
    const combined = `${readme}\n${publicApi}`;

    expect(readme).toContain("[`docs/public-api.md`](docs/public-api.md)");
    expect(readme).toContain("Install from npm once published");
    expect(readme).not.toMatch(/\bstill private\b/i);
    for (const phrase of [
      "metadata",
      "pages",
      "pageNumber",
      "label",
      "mediabox",
      "cropbox",
      "artbox",
      "bleedbox",
      "trimbox",
      "chars",
      "extractText",
      "extractTextLines",
      "extractWords",
      "search",
      "filter",
      "crop",
      "withinBbox",
      "outsideBbox",
      "dedupeChars",
      "rects",
      "lines",
      "curves",
      "`pts`",
      "`path` command",
      "document-level vector edges",
      "stroking color",
      "non-stroking color",
      "annots",
      "hyperlinks",
      "data",
      "mcid",
      "tag",
      "images",
      "srcsize",
      "imagemask",
      "colorspace",
      "bits",
      "findTables",
      "extractTables",
      "TableAxisGroup",
      "explicit_vertical_lines",
      "explicit_horizontal_lines",
      "`file:` URL",
      "ArrayBuffer",
      "Uint8Array",
      "Blob",
      "URL",
      "Chromium, Firefox, and WebKit"
    ]) {
      expect(combined, phrase).toContain(phrase);
    }
    expect(publicApi).toContain("do not choose or configure the internal PDF engine");
    expect(readme).toContain("document-level vector edges");
    expect(readme).toContain("`close()`: releases runtime resources.");
    expect(readme).not.toContain("releases PDF.js resources");
  });

  it("publishes one documented ESM surface for Node and browser consumers", () => {
    const pkg = JSON.parse(read("package.json")) as Record<string, any>;

    expect(pkg.type).toBe("module");
    expect(pkg.private).not.toBe(true);
    expect(pkg.main).toBe("./dist/src/index.js");
    expect(pkg.module).toBe("./dist/src/index.js");
    expect(pkg.browser).toBe("./dist/browser/index.js");
    expect(pkg.types).toBe("./dist/src/index.d.ts");
    expect(pkg.files).toEqual(["dist/src", "dist/browser", "docs/public-api.md"]);
    expect(pkg.exports).toEqual({
      ".": {
        types: "./dist/src/index.d.ts",
        browser: "./dist/browser/index.js",
        import: "./dist/src/index.js"
      }
    });
  });

  it("builds the browser entry as browser-targeted ESM with a packaged worker", () => {
    const pkg = JSON.parse(read("package.json")) as Record<string, any>;
    const buildBrowser = String(pkg.scripts["build:browser"]);

    expect(buildBrowser).toContain("esbuild src/index.ts");
    expect(buildBrowser).toContain("--format=esm");
    expect(buildBrowser).toContain("--platform=browser");
    expect(buildBrowser).toContain("--external:node:*");
    expect(buildBrowser).toContain("--outfile=dist/browser/index.js");
    expect(buildBrowser).toContain("pdf.worker.mjs");
    expect(buildBrowser).toContain("--outfile=dist/browser/pdf.worker.js");
  });

  it("audits the packed payload that users install", () => {
    const pkg = JSON.parse(read("package.json")) as Record<string, any>;
    const packageCheck = read("scripts/check-package-payload.mjs");

    expect(String(pkg.scripts["package:check"])).toBe("node scripts/check-package-payload.mjs");
    expect(packageCheck).toContain("packageJson.private === true");
    expect(packageCheck).toContain("must be installable and publishable");
    expect(packageCheck).toContain('"npm", ["pack", "--dry-run", "--json"]');
    for (const entrypointField of ["packageJson.main", "packageJson.module", "packageJson.browser", "packageJson.types", "collectExportPaths(packageJson.exports)"]) {
      expect(packageCheck, entrypointField).toContain(entrypointField);
    }
    expect(packageCheck).toContain("package entrypoint");
    expect(packageCheck).toContain("runInstalledImportSmoke");
    expect(packageCheck).toContain('import { open } from "pdfplumber-js";');
    expect(packageCheck).toContain("pathToFileURL(fixturePath)");
    expect(packageCheck).toContain("['ArrayBuffer', arrayBuffer.slice(0)]");
    expect(packageCheck).toContain("['Uint8Array', new Uint8Array(bytes)]");
    expect(packageCheck).toContain("['Blob', new Blob([arrayBuffer], { type: 'application/pdf' })]");
    expect(packageCheck).toContain("createServer((_request, response)");
    expect(packageCheck).toContain("['HTTP URL string', httpUrl]");
    expect(packageCheck).toContain("['HTTP URL object', new URL(httpUrl)]");
    expect(packageCheck).toContain("node_modules");
    expect(packageCheck).toContain("Object.keys(packageJson.dependencies");
    expect(packageCheck).toContain("Official - Subject to Final Review");
    expect(packageCheck).toContain("runInstalledTypeSmoke");
    expect(packageCheck).toContain('"typescript", "bin", "tsc"');
    expect(packageCheck).toContain("consumer.ts");
    expect(packageCheck).toContain("type PDFPlumberDocument");
    expect(packageCheck).toContain("type PDFPlumberPage");
    expect(packageCheck).toContain("type SearchResult");
    for (const optionType of [
      "type PDFInput",
      "type OpenOptions",
      "type ExtractTextOptions",
      "type WordOptions",
      "type SearchOptions",
      "type TextLineOptions",
      "type CropOptions",
      "type DedupeOptions",
      "type TableOptions",
      "type TableStrategy",
      "type ExplicitTableLine",
      "type TableAxisGroup"
    ]) {
      expect(packageCheck, optionType).toContain(optionType);
    }
    for (const typedAliasUse of [
      "const snakeCropped: PDFPlumberPage = page.within_bbox",
      "const snakeTable: Table | null = await snakeCropped.find_table",
      "const snakeText: string = await page.extract_text",
      "const snakeWords: PDFObject[] = await page.extract_words",
      "const snakeLines: SearchResult[] = await page.extract_text_lines",
      "await page.extract_table(tableOptions)"
    ]) {
      expect(packageCheck, typedAliasUse).toContain(typedAliasUse);
    }
    expect(packageCheck).toContain("runInstalledBrowserBundleSmoke");
    expect(packageCheck).toContain('"esbuild", "bin", "esbuild"');
    expect(packageCheck).toContain("--platform=browser");
    expect(packageCheck).toContain("browser bundler did not resolve the package through dist/browser/index.js");
    expect(packageCheck).toContain("node_modules/pdfplumber-js/dist/browser/index.js");
    expect(packageCheck).toContain("node_modules/pdfplumber-js/dist/src/index.js");
    expect(packageCheck).toContain("browser bundle does not reference the packaged pdf.worker.js asset");
    for (const requiredPath of [
      "README.md",
      "package.json",
      "docs/public-api.md",
      "dist/src/index.js",
      "dist/src/index.d.ts",
      "dist/browser/index.js",
      "dist/browser/pdf.worker.js"
    ]) {
      expect(packageCheck, requiredPath).toContain(requiredPath);
    }
    for (const forbiddenPath of [
      "dist/test/",
      "test/",
      "src/",
      "scripts/",
      "pdfplumber-python/",
      "pdfminer-six/",
      "pdfjs/",
      "playwright-report/",
      "test-results/"
    ]) {
      expect(packageCheck, forbiddenPath).toContain(forbiddenPath);
    }
  });

  it("keeps the browser compatibility gate tied to built ESM and required browsers/inputs", () => {
    const pkg = JSON.parse(read("package.json")) as Record<string, any>;
    const playwrightConfig = read("playwright.config.ts");
    const browserSpec = read("test/browser/pdfplumber.browser.spec.ts");

    expect(String(pkg.scripts["test:browser"])).toBe("npm run build && playwright test test/browser");
    expect(playwrightConfig).toContain('name: "chromium"');
    expect(playwrightConfig).toContain('name: "firefox"');
    expect(playwrightConfig).toContain('name: "webkit"');
    expect(browserSpec).toContain('pathToFileURL(path.join(repoRoot, "dist/src/index.js")).href');
    expect(browserSpec).toContain('moduleUrl: `${baseUrl}/dist/browser/index.js`');
    expect(browserSpec).toContain("arrayBuffer: await summarize(bytes.slice(0))");
    expect(browserSpec).toContain("uint8Array: await summarize(new Uint8Array(bytes.slice(0)))");
    expect(browserSpec).toContain("blob: await summarize(new Blob([bytes], { type: \"application/pdf\" }))");
    expect(browserSpec).toContain("urlObject: await summarize(new URL(pdfUrl))");
    expect(browserSpec).toContain("urlString: await summarize(pdfUrl)");
    expect(browserSpec).toContain('actual["markedTable:arrayBuffer"]');
    expect(browserSpec).toContain("expect(actual.uint8Array).toEqual(expected)");
    expect(browserSpec).toContain('actual["markedTable:blob"]');
    expect(browserSpec).toContain('actual["markedTable:url"]');
    expect(browserSpec).toContain("documentEdges");
    expect(browserSpec).toContain("document.edges.length");
    expect(browserSpec).toContain("snakeCaseAliases");
    expect(browserSpec).toContain("await page.extract_text()");
    expect(browserSpec).toContain("await page.extract_words()");
    expect(browserSpec).toContain("await page.extract_text_lines()");
    expect(browserSpec).toContain("page.within_bbox(halfBox)");
    expect(browserSpec).toContain("page.outside_bbox(halfBox)");
    expect(browserSpec).toContain("page.dedupe_chars()");
    expect(browserSpec).toContain("await page.find_tables()");
    expect(browserSpec).toContain("await page.extract_table()");
    expect(browserSpec).toContain("firstRow");
    expect(browserSpec).toContain("firstColumn");
    expect(browserSpec).toContain("pageBoxes");
    expect(browserSpec).toContain("artbox: page.artbox");
    expect(browserSpec).toContain("trimbox: page.trimbox");
    expect(browserSpec).toContain('"pts"');
    expect(browserSpec).toContain('"path"');
  });

  it("keeps backend choices out of the public option and export surface", () => {
    const indexSource = read("src/index.ts");
    const typesSource = read("src/types.ts");
    const openOptions = /export interface OpenOptions \{(?<body>[\s\S]*?)\n\}/.exec(typesSource)?.groups?.body ?? "";
    const optionNames = [...openOptions.matchAll(/^\s*([A-Za-z_]\w*)\??:/gm)].map((match) => match[1]).sort();

    expect(indexSource).not.toMatch(/\b(?:pdfjs|mupdf|backend|engine|native)\b/i);
    expect(optionNames).toEqual(["laparams", "pages", "password", "raise_unicode_errors", "unicode_norm"]);
  });

  it("keeps PDF.js scoped to named tested runtime/operator roles", () => {
    const pdfjsSourceImports = walk("src")
      .filter((file) => file.endsWith(".ts"))
      .filter((file) => read(file).includes("pdfjs-dist"))
      .sort();
    const engineDocs = read("docs/pdf-engine-comparison.md");
    const publicApiDocs = read("docs/public-api.md");
    const compatibilityDocs = read("docs/compatibility-plan.md");
    const corpusDocs = read("docs/pdf-corpus-expansion.md");
    const flatPublicApiDocs = flatText(publicApiDocs);
    const flatCompatibilityDocs = flatText(compatibilityDocs);
    const openSource = read("src/open.ts");
    const pdfSource = read("src/pdf.ts");

    expect(pdfjsSourceImports).toEqual(["src/open.ts", "src/pdf.ts"]);
    for (const phrase of [
      "Opening PDF bytes in Node and browsers",
      "Browser worker packaging and runtime asset wiring",
      "Transitional page metadata, text-content, operator-list, and annotation",
      "retrieval used as internal inputs to pdfplumber-shaped extraction",
      "Operator-code constants",
      "PDF.js is not a public engine selector or oracle",
      "`pdfjs-capability` dashboard item"
    ]) {
      expect(engineDocs, phrase).toContain(phrase);
    }
    expect(flatPublicApiDocs).toContain("PDF.js may appear internally only for named, tested runtime/operator capabilities");
    expect(flatPublicApiDocs).toContain("PDF.js is not a public engine selector or oracle");
    expect(flatCompatibilityDocs).toContain("The implementation target is native pdfminer-style parsing and extraction");
    expect(flatCompatibilityDocs).toContain("PDF.js operator-list parsing retained only for named, tested transitional roles");
    expect(corpusDocs).toContain("named PDF.js runtime/operator capabilities still used internally");
    expect(`${engineDocs}\n${compatibilityDocs}\n${corpusDocs}`).not.toMatch(
      /PDF\.js as the default dependency|default engine|starting substrate|rendering\/parsing engine that `pdfplumber\.js` depends on/
    );

    for (const symbol of [
      "GlobalWorkerOptions",
      "cMapUrl",
      "standardFontDataUrl",
      "wasmUrl",
      "getDocument",
      "getMetadata",
      "getTextContent",
      "getOperatorList",
      "getAnnotations"
    ]) {
      expect(openSource, symbol).toContain(symbol);
    }
    expect(pdfSource).toContain("pdfjs.OPS");
    for (const op of ["showText", "constructPath", "setFillRGBColor", "beginMarkedContentProps", "paintImageXObject"]) {
      expect(pdfSource, op).toContain(`pdfjs.OPS.${op}`);
    }
  });
});
