import { describe, expect, it } from "vitest";
import { createUtf8OffsetMapper } from "./markdown-offsets";

describe("Rust byte offsets mapped to CodeMirror positions", () => {
  it("maps exact Unicode byte boundaries to UTF-16 editor positions", () => {
    const source = "Aé😀Z";
    const toEditor = createUtf8OffsetMapper(source);
    expect([0, 1, 3, 7, 8].map(toEditor)).toEqual([0, 1, 2, 4, 5]);
  });

  it("maps offsets inside a multibyte scalar to the scalar start", () => {
    const toEditor = createUtf8OffsetMapper("éx");
    expect(toEditor(0)).toBe(0);
    expect(toEditor(1)).toBe(0);
    expect(toEditor(2)).toBe(1);
  });
});
