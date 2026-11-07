/**
 * Pointing at an element of the preview (SDK-46). The Portal's preview transpiler stamps every
 * HTML element with `data-jc-src="{file}:{line}"`; while the host page has switched the select
 * mode on (`{kind: "jc-select-mode", on}` from the window that framed this document), a click is
 * taken by the SDK and posted as `{kind: "jc-select", src}`: the position only, never the
 * element's text or a row. A published App carries no stamp, so a click there posts nothing.
 */

export const SOURCE_ATTRIBUTE = "data-jc-src";

export interface SelectOptions {
  win?: Window;
  doc?: Document;
}

let listening = false;
let selecting = false;

export function startSelect(options: SelectOptions = {}): void {
  if (listening) return;
  const win = options.win ?? (typeof window !== "undefined" ? window : undefined);
  const doc = options.doc ?? (typeof document !== "undefined" ? document : undefined);
  if (!win || !doc || !win.parent || win.parent === win || typeof win.addEventListener !== "function") {
    return;
  }
  listening = true;
  win.addEventListener("message", (event: MessageEvent) => {
    const data = (typeof event.data === "object" && event.data !== null ? event.data : {}) as Record<string, unknown>;
    if (event.source !== win.parent || data.kind !== "jc-select-mode") {
      return;
    }
    selecting = data.on === true;
    doc.documentElement.style.cursor = selecting ? "crosshair" : "";
  });
  doc.addEventListener(
    "click",
    (event: MouseEvent) => {
      if (!selecting) return;
      event.preventDefault();
      event.stopPropagation();
      const target = event.target as Element | null;
      const src = typeof target?.closest === "function" ? target.closest(`[${SOURCE_ATTRIBUTE}]`)?.getAttribute(SOURCE_ATTRIBUTE) : null;
      if (src) {
        win.parent.postMessage({ kind: "jc-select", src }, "*");
      }
    },
    true,
  );
}

/** Lets a test start the select mode again in the same document. */
export function resetSelect(): void {
  listening = false;
  selecting = false;
}
