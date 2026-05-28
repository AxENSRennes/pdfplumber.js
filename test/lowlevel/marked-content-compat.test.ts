import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

type MarkedSummary = Record<string, unknown>;

const objectTypes = ["chars", "lines", "curves", "rects", "images"] as const;

function markedContentOracle(paths: string[]): Record<string, MarkedSummary> {
  const code = `
import json
import pdfplumber

paths = json.loads(${JSON.stringify(JSON.stringify(paths))})
object_types = ${JSON.stringify([...objectTypes])}

def key(item):
    return f"{item.get('mcid')}|{item.get('tag')}"

def summarize_page(page):
    mcid_text = []
    for char in page.chars:
        if "mcid" not in char:
            continue
        mcid = char["mcid"]
        if mcid is None:
            continue
        while len(mcid_text) <= mcid:
            mcid_text.append("")
        if not mcid_text[mcid]:
            mcid_text[mcid] = f"{char.get('tag')}: "
        mcid_text[mcid] += char.get("text", "")
    objects = {}
    for attr in object_types:
        rows = []
        for item in getattr(page, attr):
            if "mcid" in item or "tag" in item:
                rows.append({
                    "mcid": item.get("mcid"),
                    "tag": item.get("tag"),
                    "object_type": item.get("object_type"),
                    "text": item.get("text"),
                    "name": item.get("name"),
                })
        counts = {}
        for row in rows:
            counts[key(row)] = counts.get(key(row), 0) + 1
        objects[attr] = {
            "count": len(rows),
            "counts": dict(sorted(counts.items())),
            "samples": rows[:8],
        }
    return {"mcid_text": mcid_text, "objects": objects}

out = {}
for path in paths:
    with pdfplumber.open(path) as pdf:
        out[path] = summarize_page(pdf.pages[0])
print(json.dumps(out))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as Record<string, MarkedSummary>;
}

function summarizeMarkedContent(page: Record<string, any>): MarkedSummary {
  const mcidText: string[] = [];
  for (const char of page.chars as Array<Record<string, unknown>>) {
    if (!("mcid" in char) || typeof char.mcid !== "number") continue;
    while (mcidText.length <= char.mcid) mcidText.push("");
    if (!mcidText[char.mcid]) mcidText[char.mcid] = `${char.tag}: `;
    mcidText[char.mcid] += String(char.text ?? "");
  }
  const objects = Object.fromEntries(
    objectTypes.map((attr) => {
      const rows = (page[attr] as Array<Record<string, unknown>>)
        .filter((item) => "mcid" in item || "tag" in item)
        .map((item) => ({
          mcid: item.mcid ?? null,
          tag: item.tag ?? null,
          object_type: item.object_type ?? null,
          text: item.text ?? null,
          name: item.name ?? null
        }));
      const counts = new Map<string, number>();
      for (const row of rows) {
        const key = `${row.mcid == null ? "None" : row.mcid}|${row.tag ?? null}`;
        counts.set(key, (counts.get(key) ?? 0) + 1);
      }
      return [
        attr,
        {
          count: rows.length,
          counts: Object.fromEntries([...counts.entries()].sort(([a], [b]) => a.localeCompare(b))),
          samples: rows.slice(0, 8)
        }
      ];
    })
  );
  return { mcid_text: mcidText, objects };
}

describe("low-level pdfplumber marked-content compatibility", () => {
  it("exposes mcid/tag fields on chars, vectors, and images like Python pdfplumber", async () => {
    const paths = [
      "pdfplumber-python/tests/pdfs/mcid_example.pdf",
      "pdfplumber-python/tests/pdfs/figure_structure.pdf",
      "pdfplumber-python/tests/pdfs/image_structure.pdf",
      "pdfplumber-python/tests/pdfs/2023-06-20-PV.pdf"
    ];
    const expected = markedContentOracle(paths);
    for (const path of paths) {
      const pdf = await open(path);
      try {
        expect(summarizeMarkedContent(pdf.pages[0] as unknown as Record<string, any>), path).toEqual(expected[path]);
      } finally {
        await pdf.close();
      }
    }
  }, 15_000);
});
