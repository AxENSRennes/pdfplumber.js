import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

interface ImageMetadata {
  name: unknown;
  srcsize: unknown;
  imagemask: unknown;
  bits: unknown;
  colorspace: unknown;
}

function pdfplumberImageMetadataOracle(paths: string[]): Record<string, ImageMetadata[]> {
  const code = `
import json
import pdfplumber

paths = json.loads(${JSON.stringify(JSON.stringify(paths))})
out = {}
for path in paths:
    with pdfplumber.open(path) as pdf:
        images = []
        for image in pdf.pages[0].images:
            stream = image.get("stream")
            is_inline = getattr(stream, "objid", None) is None
            images.append({
                "name": None if is_inline else image.get("name"),
                "srcsize": image.get("srcsize"),
                "imagemask": image.get("imagemask"),
                "bits": image.get("bits"),
                "colorspace": image.get("colorspace"),
            })
        out[path] = images
print(json.dumps(out, default=str))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as Record<string, ImageMetadata[]>;
}

function stableImageMetadata(image: Record<string, unknown>): ImageMetadata {
  const name = typeof image.name === "string" && image.name.startsWith("inline-image-") ? null : image.name;
  return {
    name,
    srcsize: image.srcsize,
    imagemask: image.imagemask,
    bits: image.bits,
    colorspace: image.colorspace
  };
}

describe("low-level pdfplumber image metadata compatibility", () => {
  it("exposes pdfminer-style image srcsize, masks, bits, and colorspace", async () => {
    const paths = [
      "test/fixtures/micro-pdfs/image-xobject.pdf",
      "test/fixtures/micro-pdfs/images-advanced.pdf",
      "pdfplumber-python/tests/pdfs/image_structure.pdf",
      "pdfplumber-python/tests/pdfs/issue-203-decimalize.pdf"
    ];
    const expected = pdfplumberImageMetadataOracle(paths);
    for (const path of paths) {
      const pdf = await open(path);
      try {
        expect(pdf.pages[0].images.map(stableImageMetadata), path).toEqual(expected[path]);
      } finally {
        await pdf.close();
      }
    }
  });
});
