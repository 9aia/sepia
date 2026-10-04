import { describe, expect, it } from "vite-plus/test";
import { urlBase64ToUint8Array } from "../lib/push";

const toBase64Url = (bytes: Uint8Array): string =>
  Buffer.from(bytes)
    .toString("base64")
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");

describe("urlBase64ToUint8Array", () => {
  it("decodes a base64url VAPID key (uncompressed EC point, 65 bytes)", () => {
    const bytes = new Uint8Array([4, ...Array.from({ length: 64 }, (_, i) => i)]);
    const decoded = urlBase64ToUint8Array(toBase64Url(bytes));
    expect(decoded).toEqual(bytes);
  });

  it("handles lengths needing one or two padding chars", () => {
    for (const length of [1, 2, 3, 4, 5, 86, 87]) {
      const bytes = new Uint8Array(Array.from({ length }, (_, i) => (i * 37) % 256));
      expect(urlBase64ToUint8Array(toBase64Url(bytes))).toEqual(bytes);
    }
  });

  it("accepts keys that already carry padding", () => {
    const bytes = new Uint8Array([1, 2, 3, 4, 5]);
    const withPadding = Buffer.from(bytes).toString("base64").replaceAll("+", "-");
    expect(urlBase64ToUint8Array(withPadding)).toEqual(bytes);
  });

  it("maps the base64url alphabet (- and _) correctly", () => {
    // 0xfb, 0xff, 0xfe encode to "-__-" under base64url (vs "+//+" in base64).
    const decoded = urlBase64ToUint8Array("-__-");
    expect(decoded).toEqual(new Uint8Array([0xfb, 0xff, 0xfe]));
  });
});
