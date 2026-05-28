export class CCITTG4ParserLikePdfminer {
  readonly width: number;
  protected refline: number[] = [];
  protected curline: number[] = [];
  protected curpos = -1;
  protected color = 1;

  constructor(width: number) {
    this.width = width;
    this.reset();
  }

  reset(): void {
    this.curline = Array.from({ length: this.width }, () => 1);
    this.resetLine();
  }

  resetLine(): void {
    this.refline = this.curline;
    this.curline = Array.from({ length: this.width }, () => 1);
    this.curpos = -1;
    this.color = 1;
  }

  setCurrentLine(bits: string): void {
    this.curline = [...bits].map((bit) => Number(bit));
    this.resetLine();
  }

  get curposLikePdfminer(): number {
    return this.curpos;
  }

  set curposLikePdfminer(value: number) {
    this.curpos = value;
  }

  get colorLikePdfminer(): number {
    return this.color;
  }

  set colorLikePdfminer(value: number) {
    this.color = value;
  }

  bitsLikePdfminer(): string {
    return this.curline.slice(0, this.curpos).join("");
  }

  doVerticalLikePdfminer(dx: number): void {
    let x1 = this.curpos + 1;
    while (true) {
      if (x1 === 0) {
        if (this.color === 1 && this.refline[x1] !== this.color) break;
      } else if (x1 === this.refline.length || (this.refline[x1 - 1] === this.color && this.refline[x1] !== this.color)) {
        break;
      }
      x1 += 1;
    }
    x1 += dx;
    const x0 = Math.max(0, this.curpos);
    x1 = Math.max(0, Math.min(this.width, x1));
    if (x1 < x0) {
      for (let x = x1; x < x0; x += 1) this.setLineBit(x, this.color);
    } else if (x0 < x1) {
      for (let x = x0; x < x1; x += 1) this.setLineBit(x, this.color);
    }
    this.curpos = x1;
    this.color = 1 - this.color;
  }

  doPassLikePdfminer(): void {
    let x1 = this.curpos + 1;
    while (true) {
      if (x1 === 0) {
        if (this.color === 1 && this.refline[x1] !== this.color) break;
      } else if (x1 === this.refline.length || (this.refline[x1 - 1] === this.color && this.refline[x1] !== this.color)) {
        break;
      }
      x1 += 1;
    }
    while (true) {
      if (x1 === 0) {
        if (this.color === 0 && this.refline[x1] === this.color) break;
      } else if (x1 === this.refline.length || (this.refline[x1 - 1] !== this.color && this.refline[x1] === this.color)) {
        break;
      }
      x1 += 1;
    }
    for (let x = this.curpos; x < x1; x += 1) this.setLineBit(x, this.color);
    this.curpos = x1;
  }

  doHorizontalLikePdfminer(n1: number, n2: number): void {
    if (this.curpos < 0) this.curpos = 0;
    let x = this.curpos;
    for (let i = 0; i < n1; i += 1) {
      if (this.curline.length <= x) break;
      this.setLineBit(x, this.color);
      x += 1;
    }
    for (let i = 0; i < n2; i += 1) {
      if (this.curline.length <= x) break;
      this.setLineBit(x, 1 - this.color);
      x += 1;
    }
    this.curpos = x;
  }

  protected setLineBit(index: number, value: number): void {
    const resolved = index < 0 ? this.curline.length + index : index;
    if (resolved >= 0 && resolved < this.curline.length) this.curline[resolved] = value;
  }
}

export class CCITTFaxDecoderLikePdfminer extends CCITTG4ParserLikePdfminer {
  private readonly reversed: boolean;
  private readonly buffer: number[] = [];

  constructor(width: number, options: { reversed?: boolean } = {}) {
    super(width);
    this.reversed = Boolean(options.reversed);
  }

  outputLineLikePdfminer(bits: ArrayLike<number>): void {
    const bytes = Array.from({ length: Math.ceil(bits.length / 8) }, () => 0);
    for (let i = 0; i < bits.length; i += 1) {
      const bit = this.reversed ? 1 - Number(bits[i]) : Number(bits[i]);
      if (bit) bytes[i >> 3] += [128, 64, 32, 16, 8, 4, 2, 1][i & 7];
    }
    this.buffer.push(...bytes);
  }

  closeLikePdfminer(): Uint8Array {
    return Uint8Array.from(this.buffer);
  }
}
