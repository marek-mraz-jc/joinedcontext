import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";
import { beforeEach } from "vitest";
import { settlePrefill } from "../src/assistant/state";

// A hand-off the assistant leaves lives until the person leaves the page it was left on
// (T-0907), and that outlives one test: every case starts as somebody who was handed nothing.
beforeEach(() => {
  settlePrefill("/");
});

// Four agent sandboxes share the Mac's CPU and CI runs every file at once, so the
// library's 1 s default for findBy*/waitFor is not a margin, it is a coin flip.
configure({ asyncUtilTimeout: 5_000 });

if (typeof document !== "undefined") {
  document.documentElement.lang = "sk";
}

if (typeof window !== "undefined" && !window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

// jsdom has <dialog> without its modal methods; the grid's row dialog opens with showModal (T-4828).
if (typeof HTMLDialogElement !== "undefined" && !HTMLDialogElement.prototype.showModal) {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
}
