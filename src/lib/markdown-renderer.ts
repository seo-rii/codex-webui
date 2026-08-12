import { toHtml } from "hast-util-to-html";
import { createLowlight, common } from "lowlight";
import { Marked, Renderer } from "marked";

const sharedLowlight = createLowlight(common);
const copyLabelMarker = "__CODEX_WEBUI_COPY_CODE_LABEL__";
const markdownAllowedTags = new Set([
  "a",
  "blockquote",
  "br",
  "button",
  "code",
  "div",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "hr",
  "img",
  "li",
  "ol",
  "p",
  "path",
  "pre",
  "span",
  "strong",
  "svg",
  "table",
  "tbody",
  "td",
  "th",
  "thead",
  "tr",
  "ul"
]);
const markdownGlobalAttributes = new Set(["aria-hidden", "aria-label", "class", "title"]);
const markdownTagAttributes = new Map([
  ["a", new Set(["href", "rel", "target"])],
  ["button", new Set(["data-copy-code", "data-copy-label", "type"])],
  ["img", new Set(["alt", "loading", "src"])],
  ["path", new Set(["d", "stroke-linecap", "stroke-linejoin", "stroke-width"])],
  ["span", new Set(["data-copy-code-label"])],
  ["svg", new Set(["fill", "stroke", "viewbox"])]
]);

function escapeHtml(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function safeHref(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  if (trimmed.startsWith("/") || trimmed.startsWith("./") || trimmed.startsWith("../") || trimmed.startsWith("#")) {
    return trimmed;
  }
  try {
    const parsed = new URL(trimmed, "http://localhost");
    if (["http:", "https:", "mailto:"].includes(parsed.protocol)) {
      return trimmed;
    }
  } catch {
    return null;
  }
  return null;
}

const renderer = new Renderer();

renderer.code = function ({ text: code, lang }) {
  const normalizedLanguage = lang?.trim().toLowerCase() ?? "";
  const highlighted =
    normalizedLanguage && sharedLowlight.registered(normalizedLanguage)
      ? toHtml(sharedLowlight.highlight(normalizedLanguage, code))
      : escapeHtml(code);
  const languageClass = normalizedLanguage ? ` language-${escapeHtml(normalizedLanguage)}` : "";
  const languageLabel = normalizedLanguage || "text";

  return `<div class="code-block group relative my-4 rounded-xl overflow-hidden border border-gray-200 bg-gray-50/50">
    <div class="flex items-center justify-between px-4 py-1.5 bg-gray-100/50 border-b border-gray-200 text-[10px] font-bold text-gray-500 uppercase tracking-widest">
      <span>${escapeHtml(languageLabel)}</span>
      <button aria-label="${copyLabelMarker}" class="code-copy-button" data-copy-code data-copy-label="${copyLabelMarker}" title="${copyLabelMarker}" type="button">
        <svg aria-hidden="true" class="code-copy-button__icon" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path d="M8 7a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-8a2 2 0 0 1-2-2V7Z" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8"></path>
          <path d="M16 19H6a2 2 0 0 1-2-2V7" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8"></path>
        </svg>
        <span data-copy-code-label>${copyLabelMarker}</span>
      </button>
    </div>
    <pre class="p-4 overflow-x-auto text-sm leading-relaxed"><code class="hljs${languageClass}">${highlighted}</code></pre>
  </div>`;
};

renderer.html = function ({ text: html }) {
  return `<span>${escapeHtml(html)}</span>`;
};

renderer.link = function ({ href, title, tokens }) {
  const safe = safeHref(href);
  const body = this.parser.parseInline(tokens);
  if (!safe) {
    return `<span>${body}</span>`;
  }
  const titleAttr = title ? ` title="${escapeHtml(title)}"` : "";
  const isLocal = safe.startsWith("/");
  const classes = isLocal
    ? "text-amber-600 font-medium hover:underline decoration-amber-500/30 underline-offset-4"
    : "text-amber-600 font-medium hover:underline decoration-amber-500/30 underline-offset-4 inline-flex items-center gap-0.5";
  const externalIcon = isLocal
    ? ""
    : '<svg class="w-3 h-3 opacity-60" fill="none" stroke="currentColor" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14L21 3" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"></path></svg>';
  return `<a href="${escapeHtml(safe)}" class="${classes}" rel="noreferrer" target="_blank"${titleAttr}>${body}${externalIcon}</a>`;
};

renderer.image = function ({ href, title, text: altText }) {
  const safe = safeHref(href);
  if (!safe) {
    return "";
  }
  const titleAttr = title ? ` title="${escapeHtml(title)}"` : "";
  return `<div class="my-4 overflow-hidden rounded-xl border border-gray-200 shadow-sm"><img alt="${escapeHtml(altText || "")}" class="w-full h-auto block" loading="lazy" src="${escapeHtml(safe)}"${titleAttr} /></div>`;
};

const marked = new Marked({
  breaks: true,
  gfm: true,
  renderer
});

function sanitizeRenderedMarkdown(parsed: string) {
  if (typeof document === "undefined") {
    return parsed;
  }

  const template = document.createElement("template");
  template.innerHTML = parsed;
  const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_ELEMENT);
  const elements: Element[] = [];
  let current = walker.nextNode();
  while (current) {
    elements.push(current as Element);
    current = walker.nextNode();
  }

  for (const element of elements) {
    const tagName = element.tagName.toLowerCase();
    if (!markdownAllowedTags.has(tagName)) {
      element.replaceWith(document.createTextNode(element.textContent ?? ""));
      continue;
    }

    const tagAttributes = markdownTagAttributes.get(tagName);
    for (const attribute of Array.from(element.attributes)) {
      const name = attribute.name.toLowerCase();
      const value = attribute.value;
      let keep = markdownGlobalAttributes.has(name) || Boolean(tagAttributes?.has(name));

      if (name.startsWith("on") || name === "style") {
        keep = false;
      } else if ((name === "href" || name === "src") && !safeHref(value)) {
        keep = false;
      } else if (name === "target" && value !== "_blank") {
        keep = false;
      } else if (name === "type" && tagName === "button" && value !== "button") {
        keep = false;
      } else if (name === "class" && !/^[\w\s:./,[\]()%+=-]+$/u.test(value)) {
        keep = false;
      } else if (name === "d" && !/^[ACHLQSTVZMac hlqstvz0-9,.\s-]+$/u.test(value)) {
        keep = false;
      } else if (name === "rel") {
        element.setAttribute("rel", "noreferrer");
      }

      if (!keep) {
        element.removeAttribute(attribute.name);
      }
    }
  }

  return template.innerHTML;
}

export function renderMarkdown(text: string, copyLabel: string) {
  if (!text.trim()) {
    return "";
  }
  const escapedCopyLabel = escapeHtml(copyLabel);
  const parsed = (marked.parse(text) as string).replaceAll(copyLabelMarker, () => escapedCopyLabel);
  return sanitizeRenderedMarkdown(parsed);
}
