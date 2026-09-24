/** Build a UTF-8 byte offset to JavaScript UTF-16 code-unit mapper.
 * Rust's markdown parser reports byte offsets, while CodeMirror indexes UTF-16
 * code units. Parser offsets land on Unicode scalar boundaries; offsets inside
 * a multibyte character map to that character's start.
 */
export function createUtf8OffsetMapper(source: string): (byteOffset: number) => number {
  const byteBoundaries = [0];
  const utf16Boundaries = [0];
  const encoder = new TextEncoder();
  let byteOffset = 0;
  let utf16Offset = 0;
  for (const character of source) {
    byteOffset += encoder.encode(character).length;
    utf16Offset += character.length;
    byteBoundaries.push(byteOffset);
    utf16Boundaries.push(utf16Offset);
  }

  return requestedOffset => {
    let low = 0;
    let high = byteBoundaries.length - 1;
    while (low <= high) {
      const middle = (low + high) >>> 1;
      const boundary = byteBoundaries[middle]!;
      if (boundary === requestedOffset) return utf16Boundaries[middle]!;
      if (boundary < requestedOffset) low = middle + 1;
      else high = middle - 1;
    }
    return utf16Boundaries[Math.max(0, high)]!;
  };
}
