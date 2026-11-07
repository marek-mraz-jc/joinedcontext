/**
 * The assistant's browser actions that act on the open page (T-5959, UI-59): point at a field
 * of the open form and type into the page's table filter. Navigation and prefilled forms come as
 * `navigate` events; these come as `ui` events, already checked by the server. Nothing here
 * submits: a highlighted field is focused, a filter only narrows what the table shows.
 *
 * A message is set as text (`textContent`), so markup the model or the data put in it stays
 * characters on the page.
 */
import { controlOf } from "../components/forms/resourceForm";

/** How long an action waits for its target: the page the navigation opened may still mount. */
export const WAIT_MS = 5000;
const STEP_MS = 100;
const NOTE_ID = "assistant-highlight-note";

export type UiAction =
  | { action: "highlight"; field: string; message: string }
  | { action: "filter"; query: string };

/** The action a `ui` event carries, or `null` for a payload no action reads. */
export function uiActionOf(payload: Record<string, unknown>): UiAction | null {
  if (payload.action === "highlight" && typeof payload.field === "string" && typeof payload.message === "string") {
    return { action: "highlight", field: payload.field, message: payload.message };
  }
  if (payload.action === "filter" && typeof payload.query === "string") {
    return { action: "filter", query: payload.query };
  }
  return null;
}

/** The control a manifest path names: as the form names it, or without the leading `spec.`. */
export function fieldControl(field: string): HTMLElement | null {
  const inSpec = field.startsWith("spec.") ? field.slice("spec.".length) : null;
  return controlOf(field) ?? (inSpec === null ? null : controlOf(inSpec));
}

/** The table filter of the open page: its search box, outside the assistant's own panel. */
export function tableFilter(): HTMLInputElement | null {
  const boxes = Array.from(document.querySelectorAll<HTMLInputElement>('main input[type="search"], input[type="search"]'));
  return boxes.find((box) => !box.closest("aside[data-layout]")) ?? null;
}

function point(control: HTMLElement, message: string): void {
  document.getElementById(NOTE_ID)?.remove();
  // The control pointed at before is no longer described by the note it lost (T-6230).
  document.querySelectorAll("[data-assistant-highlight]").forEach((el) => {
    el.removeAttribute("data-assistant-highlight");
    const rest = (el.getAttribute("aria-describedby") ?? "").split(/\s+/).filter((id) => id !== "" && id !== NOTE_ID);
    if (rest.length > 0) el.setAttribute("aria-describedby", rest.join(" "));
    else el.removeAttribute("aria-describedby");
  });
  const note = document.createElement("p");
  note.id = NOTE_ID;
  note.setAttribute("role", "status");
  note.className = "mt-1 rounded border border-info/30 bg-info-soft px-2 py-1 text-sm text-fg";
  note.textContent = message;
  control.setAttribute("data-assistant-highlight", "");
  const described = control.getAttribute("aria-describedby");
  control.setAttribute("aria-describedby", described ? `${described} ${NOTE_ID}` : NOTE_ID);
  control.insertAdjacentElement("afterend", note);
  control.scrollIntoView?.({ block: "center" });
  control.focus({ preventScroll: true });
}

function type(box: HTMLInputElement, query: string): void {
  // React keeps its own copy of an input's value: set it through the prototype's setter and
  // announce the input, so the page's own filter state takes it.
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(box, query);
  box.dispatchEvent(new Event("input", { bubbles: true }));
  box.focus();
}

/**
 * Runs one action once its target is on the page, waiting up to [`WAIT_MS`]. Resolves `true`
 * when it acted, `false` when the page has no such field or filter.
 */
export function runUiAction(action: UiAction, wait = WAIT_MS): Promise<boolean> {
  const find = (): HTMLElement | null => (action.action === "highlight" ? fieldControl(action.field) : tableFilter());
  return new Promise((resolve) => {
    const started = Date.now();
    const attempt = () => {
      const target = find();
      if (target) {
        if (action.action === "highlight") {
          point(target, action.message);
        } else {
          type(target as HTMLInputElement, action.query);
        }
        resolve(true);
      } else if (Date.now() - started >= wait) {
        resolve(false);
      } else {
        window.setTimeout(attempt, STEP_MS);
      }
    };
    attempt();
  });
}
