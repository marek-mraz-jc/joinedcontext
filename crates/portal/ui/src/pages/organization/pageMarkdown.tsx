import { createElement, Fragment } from "react";
import type { ReactNode } from "react";
import { safeHref } from "../../components/ui";

/**
 * The elements the catalogue's renderer writes (jc-core opendata_pages, T-7031) and the preview
 * shows. The Portal builds no markup from a string (ui_rules), so the preview answer is parsed
 * inert by DOMParser and rebuilt as React elements: an element outside this list keeps its text
 * only, no attribute but a link's `href` survives, and that one only through `safeHref`.
 */
const ELEMENTS = new Set([
  "h1", "h2", "h3", "h4", "h5", "h6", "p", "ul", "ol", "li", "a", "strong", "em", "del", "code", "pre",
  "blockquote", "table", "thead", "tbody", "tr", "th", "td", "hr", "br",
]);

/** `href` of an `a` the server sent: kept when safe, external ones open nothing of this page. */
function linkProps(element: Element): Record<string, string> {
  const href = safeHref(element.getAttribute("href"));
  if (href === undefined) return {};
  return /^(https?:|mailto:)/i.test(href) ? { href, rel: "noopener nofollow" } : { href };
}

function rebuild(node: Node, key: number): ReactNode {
  if (node.nodeType === Node.TEXT_NODE) return node.textContent;
  if (node.nodeType !== Node.ELEMENT_NODE) return null;
  const element = node as Element;
  const children = Array.from(element.childNodes, rebuild);
  const tag = element.tagName.toLowerCase();
  if (!ELEMENTS.has(tag)) return createElement(Fragment, { key }, ...children);
  // A heading of the page is a bold line here: the preview sits under the editor's own headings,
  // and the page's levels would break their order for a screen reader.
  if (/^h[1-6]$/.test(tag)) {
    return createElement("p", { key, className: tag === "h1" || tag === "h2" ? "text-title font-semibold" : "text-body font-semibold" }, ...children);
  }
  const props = tag === "a" ? linkProps(element) : {};
  return tag === "hr" || tag === "br"
    ? createElement(tag, { key })
    : createElement(tag, { key, ...props }, ...children);
}

/** The preview answer's `html` as React elements; nothing in it runs. */
export function previewElements(html: string): ReactNode[] {
  const body = new DOMParser().parseFromString(html, "text/html").body;
  return Array.from(body.childNodes, rebuild);
}

export type Format = "bold" | "heading" | "list" | "link" | "code";

export interface Edit {
  text: string;
  start: number;
  end: number;
}

/** The selection's lines, widened to whole lines. */
function lineSpan(text: string, start: number, end: number): [number, number] {
  const from = text.lastIndexOf("\n", start - 1) + 1;
  const next = text.indexOf("\n", end);
  return [from, next === -1 ? text.length : next];
}

/**
 * The toolbar's Markdown for the selection `start..end` of `text`: bold and code wrap it (code a
 * fenced block when it spans lines), a link wraps it as the link's text, a heading and a list
 * prefix each selected line. The new selection covers what the person typed or chose.
 */
export function applyFormat(text: string, start: number, end: number, format: Format): Edit {
  const chosen = text.slice(start, end);
  const wrap = (before: string, after: string, fallback: string): Edit => {
    const inner = chosen === "" ? fallback : chosen;
    return {
      text: text.slice(0, start) + before + inner + after + text.slice(end),
      start: start + before.length,
      end: start + before.length + inner.length,
    };
  };
  if (format === "bold") return wrap("**", "**", "text");
  if (format === "link") return wrap("[", "](https://)", "link text");
  if (format === "code") return chosen.includes("\n") ? wrap("```\n", "\n```", "") : wrap("`", "`", "code");
  const [from, to] = lineSpan(text, start, end);
  const prefix = format === "heading" ? "## " : "- ";
  const lines = text
    .slice(from, to)
    .split("\n")
    .map((line) => (line.startsWith(prefix) ? line : prefix + line))
    .join("\n");
  return { text: text.slice(0, from) + lines + text.slice(to), start: from, end: from + lines.length };
}
