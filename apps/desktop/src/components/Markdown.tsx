import { useMemo } from "react";
import { marked } from "marked";

// Configure: no async, breaks enabled so single newlines render as <br>.
marked.setOptions({ async: false, gfm: true, breaks: true });

/**
 * Minimal markdown renderer for LLM output (roast / chat). Renders a safe
 * subset (headings, bold/italic, code, lists, links) — marked's parser
 * outputs standard HTML tokens; we sanitize by only allowing known tags.
 */
export function Markdown({ text }: { text: string }) {
  const html = useMemo(() => {
    const raw = marked.parse(text, { async: false }) as string;
    return sanitize(raw);
  }, [text]);

  return (
    <div
      className="md-render"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

const ALLOWED_TAGS = new Set([
  "h1", "h2", "h3", "h4", "h5", "h6",
  "p", "br", "hr", "strong", "em", "del", "s",
  "ul", "ol", "li", "blockquote",
  "code", "pre",
  "a", "table", "thead", "tbody", "tr", "th", "td",
  "span", "div",
]);

/**
 * Strip all HTML tags except a whitelist, and remove all attributes except
 * href on <a> (forced to safe protocols). LLM output should never contain
 * hostile HTML, but this is cheap insurance since we use innerHTML.
 */
function sanitize(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  walk(doc.body);
  return doc.body.innerHTML;
}

function walk(node: Element) {
  const children = Array.from(node.children);
  for (const child of children) {
    const tag = child.tagName.toLowerCase();
    if (!ALLOWED_TAGS.has(tag)) {
      // Replace disallowed tags with their text content.
      child.replaceWith(...Array.from(child.childNodes));
      continue;
    }
    // Strip all attributes; re-add safe href for anchors.
    const href = child.getAttribute("href") ?? "";
    while (child.attributes.length > 0) {
      child.removeAttribute(child.attributes[0].name);
    }
    if (tag === "a" && (href.startsWith("http://") || href.startsWith("https://"))) {
      child.setAttribute("href", href);
      child.setAttribute("target", "_blank");
      child.setAttribute("rel", "noopener noreferrer");
    }
    walk(child);
  }
}
