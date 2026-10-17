/**
 * Allow-list HTML cleaner. Everything not explicitly allowed is unwrapped
 * (element dropped, text kept) or removed outright for script-like tags.
 */

const ALLOWED_TAGS = new Set([
  "p", "em", "strong", "code", "pre", "a", "ul", "li", "h3", "h4", "h5", "h6", "input", "br",
]);
const DROP_WITH_CONTENT = new Set(["script", "style", "iframe", "object", "embed", "template"]);
const ALLOWED_ATTRS: Record<string, string[]> = {
  a: ["href", "title"],
  li: ["class"],
  input: ["type", "checked", "disabled"],
};

/** Schemes permitted in href. Relative URLs are allowed. */
const SAFE_URL = /^(https?:|mailto:|\/|#)/i;

function cleanNode(node: Node, doc: Document): Node | null {
  if (node.nodeType === Node.TEXT_NODE) return doc.createTextNode(node.textContent ?? "");
  if (node.nodeType !== Node.ELEMENT_NODE) return null;
  const el = node as Element;
  const tag = el.tagName.toLowerCase();
  if (DROP_WITH_CONTENT.has(tag)) return null;

  const children = Array.from(el.childNodes)
    .map((c) => cleanNode(c, doc))
    .filter((c): c is Node => c !== null);

  if (!ALLOWED_TAGS.has(tag)) {
    const frag = doc.createDocumentFragment();
    children.forEach((c) => frag.appendChild(c));
    return frag;
  }

  const out = doc.createElement(tag);
  for (const name of ALLOWED_ATTRS[tag] ?? []) {
    const v = el.getAttribute(name);
    if (v === null) continue;
    if (name === "href" && !SAFE_URL.test(v) && v.includes(":")) continue;
    out.setAttribute(name, v);
  }
  if (tag === "a") {
    out.setAttribute("rel", "noopener noreferrer");
    out.setAttribute("target", "_blank");
  }
  children.forEach((c) => out.appendChild(c));
  return out;
}

export function cleanHtml(html: string): string {
  const doc = new DOMParser().parseFromString(`<body>${html}</body>`, "text/html");
  const container = doc.createElement("div");
  for (const child of Array.from(doc.body.childNodes)) {
    const cleaned = cleanNode(child, doc);
    if (cleaned) container.appendChild(cleaned);
  }
  return container.innerHTML;
}
