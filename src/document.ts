import type { PDFObject, PDFPlumberDocument, PDFPlumberPage } from "./types.js";
import { aggregateObjects } from "./utils.js";

export class PdfPlumberDocumentImpl implements PDFPlumberDocument {
  objects: Record<string, PDFObject[]>;
  private _annots: PDFObject[] | null = null;
  private _hyperlinks: PDFObject[] | null = null;

  constructor(
    readonly pdf: any,
    readonly metadata: Record<string, unknown>,
    readonly pages: PDFPlumberPage[]
  ) {
    this.objects = aggregateObjects(pages);
  }

  get annots(): PDFObject[] {
    this._annots ??= this.pages.flatMap((page) => page.annots);
    return this._annots;
  }

  get hyperlinks(): PDFObject[] {
    this._hyperlinks ??= this.pages.flatMap((page) => page.hyperlinks);
    return this._hyperlinks;
  }

  async close(): Promise<void> {
    await this.pdf.destroy();
  }
}
