#!/usr/bin/env node
import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, rename, rm, stat } from "node:fs/promises";
import http from "node:http";
import https from "node:https";
import path from "node:path";

const DEFAULT_MANIFESTS = [
  "test/fixtures/external-pdfs/manifest.json",
  "test/fixtures/external-holdout-pdfs/manifest.json"
];

const MAX_REDIRECTS = 8;

function argValues(name) {
  const values = [];
  for (let index = 2; index < process.argv.length; index += 1) {
    if (process.argv[index] === name && process.argv[index + 1]) {
      values.push(process.argv[index + 1]);
      index += 1;
    }
  }
  return values;
}

function hasArg(name) {
  return process.argv.includes(name);
}

function usage() {
  console.log(`Usage: node scripts/download-external-fixtures.mjs [options]

Downloads external PDF fixtures from manifest sourceUrl entries and verifies
size plus SHA-256 before leaving files in place.

Options:
  --manifest <path>  Manifest to use. Can be repeated. Defaults to both corpora.
  --id <id>          Download only one fixture id. Can be repeated.
  --force            Re-download files even when an existing file verifies.
  --dry-run          Print planned downloads without writing files.
  --help             Show this help.
`);
}

async function fileExists(filePath) {
  try {
    await stat(filePath);
    return true;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

async function sha256File(filePath) {
  const hash = createHash("sha256");
  const data = await readFile(filePath);
  hash.update(data);
  return hash.digest("hex");
}

async function verifyFile(entry) {
  const info = await stat(entry.localPath);
  if (entry.size !== undefined && info.size !== entry.size) {
    throw new Error(`${entry.localPath}: expected ${entry.size} bytes, got ${info.size}`);
  }
  const actualHash = await sha256File(entry.localPath);
  if (actualHash !== entry.sha256) {
    throw new Error(`${entry.localPath}: expected sha256 ${entry.sha256}, got ${actualHash}`);
  }
}

function requestUrl(url, redirectCount = 0) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith("https:") ? https : http;
    const request = client.get(
      url,
      {
        headers: {
          "User-Agent": "pdfplumber-js-fixture-downloader"
        }
      },
      (response) => {
        const status = response.statusCode ?? 0;
        const location = response.headers.location;
        if ([301, 302, 303, 307, 308].includes(status) && location) {
          response.resume();
          if (redirectCount >= MAX_REDIRECTS) {
            reject(new Error(`Too many redirects while downloading ${url}`));
            return;
          }
          resolve(requestUrl(new URL(location, url).toString(), redirectCount + 1));
          return;
        }
        if (status < 200 || status >= 300) {
          response.resume();
          reject(new Error(`HTTP ${status} while downloading ${url}`));
          return;
        }
        resolve(response);
      }
    );
    request.on("error", reject);
  });
}

async function downloadFile(entry) {
  await mkdir(path.dirname(entry.localPath), { recursive: true });
  const tempPath = `${entry.localPath}.tmp-${process.pid}`;
  const response = await requestUrl(entry.sourceUrl);
  const output = createWriteStream(tempPath, { flags: "wx" });

  try {
    await new Promise((resolve, reject) => {
      response.pipe(output);
      response.on("error", reject);
      output.on("error", reject);
      output.on("finish", resolve);
    });
    await verifyFile({ ...entry, localPath: tempPath });
    await rename(tempPath, entry.localPath);
  } catch (error) {
    await rm(tempPath, { force: true });
    throw error;
  }
}

async function readManifest(manifestPath) {
  const raw = await readFile(manifestPath, "utf8");
  const entries = JSON.parse(raw);
  if (!Array.isArray(entries)) throw new Error(`${manifestPath}: expected a JSON array`);
  return entries.map((entry) => ({ ...entry, manifestPath }));
}

async function main() {
  if (hasArg("--help")) {
    usage();
    return;
  }

  const manifests = argValues("--manifest");
  const selectedIds = new Set(argValues("--id"));
  const force = hasArg("--force");
  const dryRun = hasArg("--dry-run");
  const entries = (await Promise.all((manifests.length ? manifests : DEFAULT_MANIFESTS).map(readManifest))).flat();
  const selectedEntries = selectedIds.size ? entries.filter((entry) => selectedIds.has(entry.id)) : entries;

  if (!selectedEntries.length) {
    throw new Error("No manifest entries matched the requested selection");
  }

  let downloaded = 0;
  let skipped = 0;

  for (const entry of selectedEntries) {
    if (!entry.id || !entry.localPath || !entry.sourceUrl || !entry.sha256) {
      throw new Error(`${entry.manifestPath}: entry is missing id, localPath, sourceUrl, or sha256`);
    }

    const exists = await fileExists(entry.localPath);
    if (exists && !force) {
      await verifyFile(entry);
      skipped += 1;
      console.log(`ok      ${entry.localPath}`);
      continue;
    }

    if (dryRun) {
      const action = exists ? "would re-download" : "would download";
      console.log(`${action} ${entry.localPath} <- ${entry.sourceUrl}`);
      continue;
    }

    console.log(`fetch   ${entry.localPath}`);
    await downloadFile(entry);
    downloaded += 1;
  }

  console.log(`Done. downloaded=${downloaded} skipped=${skipped} dryRun=${dryRun}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
