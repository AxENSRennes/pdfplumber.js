import type { PDFObject, PDFPlumberDocument, PDFPlumberPage } from "./types.js";
import { aggregateObjects } from "./utils.js";

export class PdfPlumberDocumentImpl implements PDFPlumberDocument {
  objects: Record<string, PDFObject[]>;
  annots: PDFObject[];
  hyperlinks: PDFObject[];

  constructor(
    readonly pdf: any,
    readonly metadata: Record<string, unknown>,
    readonly pages: PDFPlumberPage[]
  ) {
    this.objects = aggregateObjects(pages);
    this.annots = pages.flatMap((page) => page.annots);
    this.hyperlinks = pages.flatMap((page) => page.hyperlinks);
  }

  async close(): Promise<void> {
    await this.pdf.destroy();
  }
}
