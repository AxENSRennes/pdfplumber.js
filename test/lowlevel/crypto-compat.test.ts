import { describe, expect, it } from "vitest";

import { rc4BytesLikePdfminer, unpadAesLikePdfminer } from "../../src/pdf/security.js";

function bytes(value: string): Uint8Array {
  return Buffer.from(value, "latin1");
}

function hex(value: Uint8Array): string {
  return Buffer.from(value).toString("hex");
}

function latin1(value: Uint8Array): string {
  return Buffer.from(value).toString("latin1");
}

describe("low-level PDF crypto compatibility", () => {
  it("processes RC4 bytes like pdfminer.arcfour.Arcfour", () => {
    expect(hex(rc4BytesLikePdfminer(bytes("Key"), bytes("Plaintext")))).toBe("bbf316e8d940af0ad3");
    expect(hex(rc4BytesLikePdfminer(bytes("Wiki"), bytes("pedia")))).toBe("1021bf0420");
    expect(hex(rc4BytesLikePdfminer(bytes("Secret"), bytes("Attack at dawn")))).toBe("45a01f645fc35b383552544b9bf5");
  });

  it("unpads AES blocks like pdfminer.utils.unpad_aes", () => {
    expect(unpadAesLikePdfminer(Uint8Array.from({ length: 16 }, () => 0x10))).toHaveLength(0);
    expect(latin1(unpadAesLikePdfminer(bytes(`0123456789abcdef${"\x10".repeat(16)}`)))).toBe("0123456789abcdef");
    expect(latin1(unpadAesLikePdfminer(bytes("0123456789abc\x03\x03\x03")))).toBe("0123456789abc");
    expect(latin1(unpadAesLikePdfminer(bytes("0123456789abcdef0123456789abc\x03\x03\x03")))).toBe("0123456789abcdef0123456789abc");
    expect(latin1(unpadAesLikePdfminer(bytes("foo\x01bar\x01bazquux\x01")))).toBe("foo\x01bar\x01bazquux");
    expect(latin1(unpadAesLikePdfminer(bytes("0123456789abc\x02\x03\x04")))).toBe("0123456789abc\x02\x03\x04");
    expect(latin1(unpadAesLikePdfminer(bytes("0123456789abc\x05\x05\x05")))).toBe("0123456789abc\x05\x05\x05");
  });
});
