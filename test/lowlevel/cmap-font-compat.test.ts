import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

interface CMapFontFixtureOracle {
  chars: number;
  hboxes: string[];
  hcount: number;
  vcount: number;
}

function pdfminerCMapFontFixtureOracle(path: string): CMapFontFixtureOracle {
  const code = `
import json
from pdfminer.high_level import extract_pages
from pdfminer.layout import LAParams, LTChar, LTContainer, LTTextBoxHorizontal, LTTextBoxVertical

def get_chars(el):
    if isinstance(el, LTContainer):
        for item in el:
            yield from get_chars(item)
    elif isinstance(el, LTChar):
        yield el

page = next(extract_pages(
    ${JSON.stringify(path)},
    laparams=LAParams(detect_vertical=True, char_margin=1, all_texts=True, boxes_flow=None),
))
hboxes = [textbox.get_text() for textbox in page if isinstance(textbox, LTTextBoxHorizontal)]
vboxes = [textbox.get_text() for textbox in page if isinstance(textbox, LTTextBoxVertical)]
print(json.dumps({
    "chars": sum(1 for _ in get_chars(page)),
    "hboxes": hboxes[:10],
    "hcount": len(hboxes),
    "vcount": len(vboxes),
}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as CMapFontFixtureOracle;
}

describe("low-level pdfminer CMap font fixture compatibility", () => {
  it("aggregates issue-598 CMap fonts with pdfminer-compatible layout output", async () => {
    const path = "pdfminer-six/samples/contrib/issue-598-cmap-other-fonts.pdf";
    const expected = pdfminerCMapFontFixtureOracle(path);
    const pdf = await open(path, { laparams: { detect_vertical: true, char_margin: 1, all_texts: true, boxes_flow: null } });
    try {
      expect({
        chars: pdf.pages[0].chars.length,
        hboxes: (pdf.pages[0].objects.textboxhorizontal ?? []).map((textbox) => String(textbox.text ?? "")).slice(0, 10),
        hcount: pdf.pages[0].objects.textboxhorizontal?.length ?? 0,
        vcount: pdf.pages[0].objects.textboxvertical?.length ?? 0
      }).toEqual(expected);
    } finally {
      await pdf.close();
    }
  });
});
