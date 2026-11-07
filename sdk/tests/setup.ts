import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";

// AppShell names the page's language and title on the document (T-3577); one test's shell must
// not decide the language the next test's components read from it.
afterEach(() => {
  if (typeof document === "undefined") return; // a node-environment file
  document.documentElement.removeAttribute("lang");
  document.title = "";
});

// jsdom has <dialog> without its modal methods; the grid's row dialog opens with showModal (T-4828).
if (typeof HTMLDialogElement !== "undefined" && !HTMLDialogElement.prototype.showModal) {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
}
