// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { footnoteBody, footnoteDomId, renderMarkdownHtml } from "./markdown-renderer";

describe("Markdown rendering contract", () => {
  it("uses GFM soft line breaks without inventing hard breaks", async () => {
    const html = await renderMarkdownHtml("first line\nsecond line");
    expect(html).toContain("first line\nsecond line");
    expect(html).not.toContain("<br");
  });

  it("keeps escaped Markdown punctuation literal after parsing", async () => {
    const html = await renderMarkdownHtml(String.raw`A \*literal\* and \_text\_.`);
    expect(html).toContain("*literal*");
    expect(html).toContain("_text_");
    expect(html).not.toContain("<em>");
  });

  it("renders lists, tables, and fenced code using the original source", async () => {
    const html = await renderMarkdownHtml("- one\n- two\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n```js\nrun()\n```");
    expect(html).toContain("<ul>");
    expect(html).toContain("<table>");
    expect(html).toContain("<code class=\"language-js\">");
  });

  it("keeps reference links working when blocks render separately", async () => {
    const html = await renderMarkdownHtml("[Index][home]", [], ["[home]: ../index.md"]);
    expect(html).toContain('<a href="../index.md">Index</a>');
  });

  it("turns known GFM footnote references into safe in-page links", async () => {
    const html = await renderMarkdownHtml("See[^note].", ["note"]);
    const root = document.createElement("div");
    root.innerHTML = html;
    const link = root.querySelector(".footnote-ref a");
    expect(link?.textContent).toBe("note");
    expect(link?.getAttribute("href")).toBe(`#${footnoteDomId("note")}`);
    expect(await renderMarkdownHtml("Literal[^missing].", ["note"])).toContain("[^missing]");
  });

  it("extracts footnote definition content and continuation lines", () => {
    expect(footnoteBody("[^note]: First line\n    continuation\n\n    final line"))
      .toBe("First line\ncontinuation\n\nfinal line");
  });
});
