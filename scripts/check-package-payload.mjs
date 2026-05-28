#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const requiredFiles = new Set([
  "README.md",
  "package.json",
  "docs/public-api.md",
  "dist/src/index.js",
  "dist/src/index.d.ts",
  "dist/browser/index.js",
  "dist/browser/pdf.worker.js"
]);

const forbiddenPrefixes = [
  "dist/test/",
  "test/",
  "src/",
  "scripts/",
  "pdfplumber-python/",
  "pdfminer-six/",
  "pdfjs/",
  "playwright-report/",
  "test-results/"
];

function fail(message) {
  console.error(`package payload check failed: ${message}`);
  process.exit(1);
}

function packagePath(specifier) {
  if (typeof specifier !== "string" || !specifier.startsWith("./")) {
    return null;
  }
  return specifier.slice(2);
}

function collectExportPaths(value, paths = []) {
  if (typeof value === "string") {
    paths.push(value);
    return paths;
  }
  if (value && typeof value === "object") {
    for (const child of Object.values(value)) {
      collectExportPaths(child, paths);
    }
  }
  return paths;
}

function copyPayloadFile(repoRoot, packageRoot, relativePath) {
  const source = path.join(repoRoot, relativePath);
  const target = path.join(packageRoot, relativePath);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.copyFileSync(source, target);
}

function linkDependency(repoRoot, nodeModulesRoot, dependencyName) {
  const source = path.join(repoRoot, "node_modules", dependencyName);
  if (!fs.existsSync(source)) {
    fail(`dependency ${dependencyName} is not installed; run npm install first.`);
  }
  const target = path.join(nodeModulesRoot, dependencyName);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.symlinkSync(source, target, "junction");
}

function runInstalledImportSmoke(repoRoot, packageJson, files) {
  const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pdfplumber-js-package-"));
  try {
    const nodeModulesRoot = path.join(tempRoot, "node_modules");
    const packageRoot = path.join(nodeModulesRoot, packageJson.name);
    for (const file of files) {
      copyPayloadFile(repoRoot, packageRoot, file);
    }
    for (const dependencyName of Object.keys(packageJson.dependencies ?? {})) {
      linkDependency(repoRoot, nodeModulesRoot, dependencyName);
    }

    const smokePath = path.join(tempRoot, "installed-import-smoke.mjs");
    fs.writeFileSync(
      smokePath,
      [
        "import { createServer } from 'node:http';",
        "import { readFile } from 'node:fs/promises';",
        "import { pathToFileURL } from 'node:url';",
        'import { open } from "pdfplumber-js";',
        "const fixturePath = process.argv[2];",
        "const bytes = await readFile(fixturePath);",
        "const arrayBuffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);",
        "const server = createServer((_request, response) => {",
        "  response.setHeader('content-type', 'application/pdf');",
        "  response.end(bytes);",
        "});",
        "await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));",
        "const address = server.address();",
        "if (!address || typeof address === 'string') throw new Error('failed to start installed-package HTTP fixture server');",
        "const httpUrl = `http://127.0.0.1:${address.port}/fixture.pdf`;",
        "const inputs = [",
        "  ['filesystem path', fixturePath],",
        "  ['file URL object', pathToFileURL(fixturePath)],",
        "  ['file URL string', pathToFileURL(fixturePath).href],",
        "  ['ArrayBuffer', arrayBuffer.slice(0)],",
        "  ['Uint8Array', new Uint8Array(bytes)],",
        "  ['Blob', new Blob([arrayBuffer], { type: 'application/pdf' })],",
        "  ['HTTP URL string', httpUrl],",
        "  ['HTTP URL object', new URL(httpUrl)]",
        "];",
        "try {",
        "  for (const [name, input] of inputs) {",
        "    const document = await open(input);",
        "    try {",
        "      const page = document.pages[0];",
        "      const text = String(await page.extractText());",
        "      const words = await page.extractWords();",
        "      if (document.pages.length !== 1) throw new Error(`${name}: expected one page, got ${document.pages.length}`);",
        "      if (!text.includes('Official - Subject to Final Review')) throw new Error(`${name}: missing expected extracted text`);",
        "      if (words.length < 100) throw new Error(`${name}: expected many words, got ${words.length}`);",
        "    } finally {",
        "      await document.close();",
        "    }",
        "  }",
        "} finally {",
        "  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));",
        "}"
      ].join("\n")
    );
    const fixturePath = path.join(repoRoot, "pdfplumber-python", "tests", "pdfs", "scotus-transcript-p1.pdf");
    const smoke = spawnSync(process.execPath, [smokePath, fixturePath], {
      cwd: tempRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"]
    });
    if (smoke.status !== 0) {
      const detail = `${smoke.stderr || smoke.stdout}`.trim();
      fail(`installed package import smoke failed${detail ? `\n${detail}` : ""}`);
    }
  } finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
  }
}

function runInstalledTypeSmoke(repoRoot, packageJson, files) {
  const tscPath = path.join(repoRoot, "node_modules", "typescript", "bin", "tsc");
  if (!fs.existsSync(tscPath)) {
    fail("typescript is not installed; run npm install first.");
  }

  const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pdfplumber-js-types-"));
  try {
    const nodeModulesRoot = path.join(tempRoot, "node_modules");
    const packageRoot = path.join(nodeModulesRoot, packageJson.name);
    for (const file of files) {
      copyPayloadFile(repoRoot, packageRoot, file);
    }
    for (const dependencyName of Object.keys(packageJson.dependencies ?? {})) {
      linkDependency(repoRoot, nodeModulesRoot, dependencyName);
    }

    fs.writeFileSync(
      path.join(tempRoot, "tsconfig.json"),
      JSON.stringify(
        {
          compilerOptions: {
            target: "ES2022",
            module: "NodeNext",
            moduleResolution: "NodeNext",
            strict: true,
            noEmit: true,
            skipLibCheck: true,
            lib: ["ES2022", "DOM"]
          },
          include: ["consumer.ts"]
        },
        null,
        2
      )
    );
    fs.writeFileSync(
      path.join(tempRoot, "consumer.ts"),
      [
        'import { Table, open, type BBox, type CropOptions, type DedupeOptions, type ExplicitTableLine, type ExtractTextOptions, type OpenOptions, type PDFInput, type PDFObject, type PDFPlumberDocument, type PDFPlumberPage, type SearchOptions, type SearchResult, type TableAxisGroup, type TableOptions, type TableStrategy, type TextLineOptions, type WordOptions } from "pdfplumber-js";',
        "const inputs: PDFInput[] = [new ArrayBuffer(0), new Uint8Array(), new Blob(), new URL('https://example.test/file.pdf')];",
        "const bbox: BBox = [0, 0, 1, 1];",
        "const openOptions: OpenOptions = { pages: [1], unicode_norm: 'NFC' };",
        "const textOptions: ExtractTextOptions = { layout: true, x_tolerance: 2, y_tolerance: 2, line_dir_render: 'ttb', char_dir_render: 'ltr', dedupe_chars: true };",
        "const wordOptions: WordOptions = { return_chars: true, split_at_punctuation: true, extra_attrs: ['fontname', 'size'] };",
        "const searchOptions: SearchOptions = { regex: true, return_chars: true, return_groups: false };",
        "const lineOptions: TextLineOptions = { strip: true, return_chars: true };",
        "const cropOptions: CropOptions = { strict: false, relative: false };",
        "const dedupeOptions: DedupeOptions = { tolerance: 1, extra_attrs: ['fontname', 'size'] };",
        "const strategy: TableStrategy = 'explicit';",
        "const explicitLines: ExplicitTableLine[] = [];",
        "const tableOptions: TableOptions = { vertical_strategy: strategy, horizontal_strategy: 'lines', explicit_vertical_lines: explicitLines, text_x_tolerance: 2 };",
        "async function consume(input: PDFInput): Promise<Array<Array<string | null>> | null> {",
        "  const document: PDFPlumberDocument = await open(input, openOptions);",
        "  try {",
        "    const page: PDFPlumberPage = document.pages[0];",
        "    const text: string = await page.extractText(textOptions);",
        "    const words: PDFObject[] = await page.extractWords(wordOptions);",
        "    const matches: SearchResult[] = await page.search(/Final/, searchOptions);",
        "    const lines: SearchResult[] = await page.extractTextLines(lineOptions);",
        "    const cropped: PDFPlumberPage = page.crop(bbox, cropOptions).withinBbox(bbox, cropOptions).outsideBbox(bbox, cropOptions).dedupeChars(dedupeOptions);",
        "    const snakeCropped: PDFPlumberPage = page.within_bbox(bbox, cropOptions).outside_bbox(bbox, cropOptions).dedupe_chars(dedupeOptions);",
        "    const table: Table | null = await cropped.findTable({ ...tableOptions, explicit_vertical_lines: page.edges });",
        "    const snakeTable: Table | null = await snakeCropped.find_table(tableOptions);",
        "    const rows: TableAxisGroup[] = table?.rows ?? [];",
        "    const snakeText: string = await page.extract_text(textOptions);",
        "    const snakeWords: PDFObject[] = await page.extract_words(wordOptions);",
        "    const snakeLines: SearchResult[] = await page.extract_text_lines(lineOptions);",
        "    void [text, words, matches, lines, inputs, rows, snakeText, snakeWords, snakeLines, snakeTable];",
        "    return table ? table.extract({ x_tolerance: 2 }) : await page.extract_table(tableOptions);",
        "  } finally {",
        "    await document.close();",
        "  }",
        "}"
      ].join("\n")
    );

    const typecheck = spawnSync(process.execPath, [tscPath, "-p", "tsconfig.json"], {
      cwd: tempRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"]
    });
    if (typecheck.status !== 0) {
      const detail = `${typecheck.stderr || typecheck.stdout}`.trim();
      fail(`installed package TypeScript consumer check failed${detail ? `\n${detail}` : ""}`);
    }
  } finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
  }
}

function runInstalledBrowserBundleSmoke(repoRoot, packageJson, files) {
  const esbuildPath = path.join(repoRoot, "node_modules", "esbuild", "bin", "esbuild");
  if (!fs.existsSync(esbuildPath)) {
    fail("esbuild is not installed; run npm install first.");
  }

  const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pdfplumber-js-browser-"));
  try {
    const nodeModulesRoot = path.join(tempRoot, "node_modules");
    const packageRoot = path.join(nodeModulesRoot, packageJson.name);
    for (const file of files) {
      copyPayloadFile(repoRoot, packageRoot, file);
    }
    for (const dependencyName of Object.keys(packageJson.dependencies ?? {})) {
      linkDependency(repoRoot, nodeModulesRoot, dependencyName);
    }

    const consumerPath = path.join(tempRoot, "browser-consumer.js");
    const bundlePath = path.join(tempRoot, "browser-bundle.js");
    const metafilePath = path.join(tempRoot, "browser-meta.json");
    fs.writeFileSync(
      consumerPath,
      [
        'import { open } from "pdfplumber-js";',
        "export async function summarize(input) {",
        "  const document = await open(input);",
        "  try {",
        "    const page = document.pages[0];",
        "    return { pages: document.pages.length, text: String(await page.extractText()).slice(0, 32) };",
        "  } finally {",
        "    await document.close();",
        "  }",
        "}"
      ].join("\n")
    );

    const bundle = spawnSync(esbuildPath, [
      consumerPath,
      "--bundle",
      "--format=esm",
      "--platform=browser",
      "--target=es2022",
      `--outfile=${bundlePath}`,
      `--metafile=${metafilePath}`,
      "--log-level=warning"
    ], {
      cwd: tempRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"]
    });
    if (bundle.status !== 0) {
      const detail = `${bundle.stderr || bundle.stdout}`.trim();
      fail(`installed browser bundle check failed${detail ? `\n${detail}` : ""}`);
    }

    const metafile = JSON.parse(fs.readFileSync(metafilePath, "utf8"));
    const inputs = Object.keys(metafile.inputs ?? {}).map((input) => input.split(path.sep).join("/"));
    const browserEntry = inputs.find((input) => input.endsWith("node_modules/pdfplumber-js/dist/browser/index.js"));
    if (!browserEntry) {
      fail(`browser bundler did not resolve the package through dist/browser/index.js:\n${inputs.join("\n")}`);
    }
    const nodeEntry = inputs.find((input) => input.endsWith("node_modules/pdfplumber-js/dist/src/index.js"));
    if (nodeEntry) {
      fail(`browser bundler resolved the Node entry instead of the browser export: ${nodeEntry}`);
    }

    const bundledSource = fs.readFileSync(bundlePath, "utf8");
    if (!bundledSource.includes("pdf.worker.js")) {
      fail("browser bundle does not reference the packaged pdf.worker.js asset");
    }
    for (const forbiddenToken of ["node:fs", "node:path", "node:module", "node:url", "node:worker_threads"]) {
      if (bundledSource.includes(forbiddenToken)) {
        fail(`browser bundle contains Node-only import token ${forbiddenToken}`);
      }
    }
  } finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
  }
}

const repoRoot = process.cwd();
const packageJson = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));
if (packageJson.private === true) {
  fail("package.json has private: true, but the stable package must be installable and publishable.");
}
const pack = spawnSync("npm", ["pack", "--dry-run", "--json"], {
  cwd: repoRoot,
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"]
});

if (pack.status !== 0) {
  const detail = `${pack.stderr || pack.stdout}`.trim();
  fail(`npm pack --dry-run failed${detail ? `\n${detail}` : ""}`);
}

let payload;
try {
  payload = JSON.parse(pack.stdout);
} catch (error) {
  fail(`npm pack did not return JSON: ${error instanceof Error ? error.message : String(error)}`);
}

const [entry] = payload;
if (!entry || !Array.isArray(entry.files)) {
  fail("npm pack output did not include a files list. Run npm run build first.");
}

const files = entry.files.map((file) => file.path).sort();
const fileSet = new Set(files);

for (const requiredFile of requiredFiles) {
  if (!fileSet.has(requiredFile)) {
    fail(`missing required file ${requiredFile}. Run npm run build first.`);
  }
}

const packageEntrypoints = [
  packageJson.main,
  packageJson.module,
  packageJson.browser,
  packageJson.types,
  ...collectExportPaths(packageJson.exports)
]
  .map(packagePath)
  .filter(Boolean);

for (const entrypoint of new Set(packageEntrypoints)) {
  if (!fileSet.has(entrypoint)) {
    fail(`package entrypoint ${entrypoint} is not included in the packed payload. Run npm run build first.`);
  }
}

const forbiddenFiles = files.filter((file) => {
  if (forbiddenPrefixes.some((prefix) => file.startsWith(prefix))) {
    return true;
  }
  if (file.endsWith(".ts") || file.endsWith(".tsx")) {
    return !file.endsWith(".d.ts");
  }
  return false;
});

if (forbiddenFiles.length > 0) {
  fail(`unexpected development files in package:\n${forbiddenFiles.join("\n")}`);
}

runInstalledImportSmoke(repoRoot, packageJson, files);
runInstalledTypeSmoke(repoRoot, packageJson, files);
runInstalledBrowserBundleSmoke(repoRoot, packageJson, files);

console.log(`package payload check passed (${files.length} files)`);
