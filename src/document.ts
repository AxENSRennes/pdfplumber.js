import type { PDFObject, PDFPlumberDocument, PDFPlumberPage } from "./types.js";
import { aggregateObjects } from "./utils.js";

export class PdfPlumberDocumentImpl implements PDFPlumberDocument {
  objects: Record<string, PDFObject[]>;
  edges: PDFObject[];
  horizontal_edges: PDFObject[];
  vertical_edges: PDFObject[];
  private _annots: PDFObject[] | null = null;
  private _hyperlinks: PDFObject[] | null = null;

  constructor(
    readonly pdf: any,
    readonly metadata: Record<string, unknown>,
    readonly pages: PDFPlumberPage[],
    private readonly closeMode: "cleanup" | "destroy" = "destroy"
  ) {
    this.objects = aggregateObjects(pages);
    this.edges = pages.flatMap((page) => page.edges);
    this.horizontal_edges = this.edges.filter((edge) => edge.orientation === "h");
    this.vertical_edges = this.edges.filter((edge) => edge.orientation === "v");
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
    if (this.closeMode === "cleanup" && typeof this.pdf.cleanup === "function") {
      await this.pdf.cleanup();
      return;
    }
    await this.pdf.destroy();
  }
}
