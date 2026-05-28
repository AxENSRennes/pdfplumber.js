import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

type TextboxKind = "horizontal" | "vertical";

interface LayoutOracle {
  horizontal: string[];
  vertical: string[];
}

function pdfminerLayoutOracle(path: string, laparams: Record<string, unknown> = {}): LayoutOracle {
  const code = `
import json
from pdfminer.high_level import extract_pages
from pdfminer.layout import LAParams, LTTextBoxHorizontal, LTTextBoxVertical

laparams = json.loads(${JSON.stringify(JSON.stringify(laparams))})
page = next(extract_pages(${JSON.stringify(path)}, laparams=LAParams(**laparams)))
print(json.dumps({
    "horizontal": [textbox.get_text() for textbox in page if isinstance(textbox, LTTextBoxHorizontal)],
    "vertical": [textbox.get_text() for textbox in page if isinstance(textbox, LTTextBoxVertical)],
}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as LayoutOracle;
}

async function jsTextboxes(path: string, laparams: Record<string, unknown>, kind: TextboxKind): Promise<string[]> {
  const pdf = await open(path, { laparams });
  try {
    const key = kind === "horizontal" ? "textboxhorizontal" : "textboxvertical";
    return (pdf.pages[0].objects[key] ?? []).map((textbox) => String(textbox.text ?? ""));
  } finally {
    await pdf.close();
  }
}

describe("low-level pdfminer layout compatibility", () => {
  it("groups simple4 text boxes like pdfminer for line_margin and boxes_flow", async () => {
    const path = "pdfminer-six/samples/simple4.pdf";
    for (const laparams of [{ line_margin: 0.19 }, { line_margin: 0.21 }, { boxes_flow: null }]) {
      const expected = pdfminerLayoutOracle(path, laparams);
      expect(await jsTextboxes(path, laparams, "horizontal")).toEqual(expected.horizontal);
    }
  });

  it("keeps issue 449 empty-character text boxes separated like pdfminer", async () => {
    const horizontal = "pdfminer-six/samples/contrib/issue-449-horizontal.pdf";
    expect(await jsTextboxes(horizontal, {}, "horizontal")).toEqual(pdfminerLayoutOracle(horizontal).horizontal);

    const vertical = "pdfminer-six/samples/contrib/issue-449-vertical.pdf";
    const verticalParams = { detect_vertical: true };
    expect(await jsTextboxes(vertical, verticalParams, "vertical")).toEqual(pdfminerLayoutOracle(vertical, verticalParams).vertical);
  });
});
