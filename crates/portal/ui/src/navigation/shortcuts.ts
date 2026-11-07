/**
 * The Portal's keyboard shortcuts (UI-92, T-3242): one small set, the same on every page. A
 * shortcut acts on what the page offers, which a page marks with `data-shortcut`, so "?" lists
 * exactly what works where the person is. A letter is never taken from a text field.
 */

/** Asks the assistant dock to open, with what was typed (UI-88): the dock listens for it. */
export const ASSISTANT_EVENT = "jc:assistant";

export type ShortcutId = "palette" | "save" | "new" | "test" | "next" | "previous" | "close" | "help";

/** In the order "?" lists them; `keys` is what the person presses, as the help writes it. */
export const SHORTCUTS: { id: ShortcutId; keys: string[] }[] = [
  { id: "palette", keys: ["Ctrl", "K"] },
  { id: "save", keys: ["Ctrl", "S"] },
  { id: "new", keys: ["N"] },
  { id: "test", keys: ["T"] },
  { id: "next", keys: ["J"] },
  { id: "previous", keys: ["K"] },
  { id: "close", keys: ["Esc"] },
  { id: "help", keys: ["?"] },
];

/** Whether the key went to something the person types into. */
export function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.isContentEditable ||
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
  );
}

/**
 * The form Ctrl+S saves: the one the focus is in, else the open dialog's, else the page's. An open
 * dialog without a form saves nothing behind it: a selector list would answer in document order,
 * and the page's `main form` comes before the dialog portaled to the end of the body.
 */
function formToSave(doc: Document): HTMLFormElement | null {
  const active = doc.activeElement;
  const around = active instanceof HTMLElement ? active.closest("form") : null;
  if (around) return around;
  const dialog = doc.querySelector("[role=dialog]");
  return dialog ? dialog.querySelector("form") : doc.querySelector<HTMLFormElement>("main form");
}

function marked(doc: Document, id: "save" | "new" | "test"): HTMLElement | null {
  const scope = doc.querySelector("[role=dialog]") ?? doc;
  return scope.querySelector<HTMLElement>(`[data-shortcut="${id}"]:not(:disabled)`);
}

/** The rows of the page's lists, each the link `RecordLink` renders for it. */
function rows(doc: Document): HTMLElement[] {
  return [...doc.querySelectorAll<HTMLElement>("main [data-row-link]")];
}

/** The shortcuts that do something on the page as it is now. */
export function available(doc: Document = document): ShortcutId[] {
  return SHORTCUTS.map((s) => s.id).filter((id) => {
    switch (id) {
      case "save":
        return Boolean(formToSave(doc) ?? marked(doc, "save"));
      case "new":
      case "test":
        return Boolean(marked(doc, id));
      case "next":
      case "previous":
        return rows(doc).length > 0;
      default:
        return true;
    }
  });
}

/**
 * Does what a key asks on the page and answers whether it did anything; the palette's and the
 * help's own keys are their owner's. Escape is every dialog's own (Radix closes it).
 */
export function act(event: KeyboardEvent, doc: Document = document): boolean {
  const ctrl = event.ctrlKey || event.metaKey;
  if (ctrl && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "s") {
    const form = formToSave(doc);
    if (form) {
      form.requestSubmit();
      return true;
    }
    const button = marked(doc, "save");
    button?.click();
    return Boolean(button);
  }
  if (ctrl || event.altKey || isTyping(event.target)) return false;
  switch (event.key) {
    case "n":
    case "t": {
      const button = marked(doc, event.key === "n" ? "new" : "test");
      button?.click();
      return Boolean(button);
    }
    case "j":
    case "k": {
      const list = rows(doc);
      if (list.length === 0) return false;
      const at = list.indexOf(doc.activeElement as HTMLElement);
      const next = event.key === "j" ? Math.min(at + 1, list.length - 1) : at < 0 ? 0 : Math.max(at - 1, 0);
      list[next].focus();
      list[next].scrollIntoView({ block: "nearest" });
      return true;
    }
    default:
      return false;
  }
}
