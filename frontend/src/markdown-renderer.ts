import DOMPurify from "dompurify";
import { marked } from "marked";

const markdownOptions = { gfm: true, breaks: false } as const;

function footnoteKey(label: string) {
  return label.trim().toLowerCase().replace(/\s+/g, " ");
}

function footnoteId(label: string) {
  return `fn-${encodeURIComponent(footnoteKey(label))}`;
}

function linkFootnoteReferences(html: string, definitions: ReadonlySet<string>) {
  if (definitions.size === 0 || typeof document === "undefined") return html;
  const root = document.createElement("div");
  root.innerHTML = html;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const textNodes: Text[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (node.parentElement?.closest("a, code, pre, script, style")) continue;
    if (/\[\^[^\]]+\]/.test(node.data)) textNodes.push(node);
  }
  for (const node of textNodes) {
    const expression = /\[\^([^\]]+)\]/g;
    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let match: RegExpExecArray | null;
    while ((match = expression.exec(node.data))) {
      const label = match[1]!;
      if (!definitions.has(footnoteKey(label))) continue;
      fragment.append(document.createTextNode(node.data.slice(cursor, match.index)));
      const sup = document.createElement("sup");
      sup.className = "footnote-ref";
      const link = document.createElement("a");
      link.href = `#${footnoteId(label)}`;
      link.textContent = label;
      sup.append(link);
      fragment.append(sup);
      cursor = match.index + match[0].length;
    }
    if (cursor === 0) continue;
    fragment.append(document.createTextNode(node.data.slice(cursor)));
    node.replaceWith(fragment);
  }
  return root.innerHTML;
}

function withReferenceDefinitions(source: string, definitions: readonly string[]) {
  return definitions.length === 0 ? source : `${source}\n\n${definitions.join("\n")}`;
}

export async function renderMarkdownHtml(
  source: string,
  footnoteLabels: readonly string[] = [],
  linkDefinitions: readonly string[] = [],
) {
  const withDefinitions = withReferenceDefinitions(source, linkDefinitions);
  const sanitized = DOMPurify.sanitize(await marked.parse(withDefinitions, markdownOptions));
  return linkFootnoteReferences(sanitized, new Set(footnoteLabels.map(footnoteKey)));
}

export async function renderInlineMarkdownHtml(
  source: string,
  footnoteLabels: readonly string[] = [],
  linkDefinitions: readonly string[] = [],
) {
  const blockHtml = await renderMarkdownHtml(source, footnoteLabels, linkDefinitions);
  const root = document.createElement("div");
  root.innerHTML = blockHtml;
  const paragraph = root.querySelector(":scope > p");
  return paragraph ? paragraph.innerHTML : root.innerHTML;
}

export function footnoteBody(source: string) {
  const lines = source.split(/\r?\n/);
  if (lines.length === 0) return "";
  lines[0] = lines[0]!.replace(/^\s*\[\^[^\]]+\]:[ \t]?/, "");
  for (let index = 1; index < lines.length; index += 1) {
    lines[index] = lines[index]!.replace(/^(?: {1,4}|\t)/, "");
  }
  return lines.join("\n").trimEnd();
}

export function footnoteDomId(label: string) {
  return footnoteId(label);
}
