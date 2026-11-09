import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";
import { beforeEach, vi } from "vitest";
import { EXACT_PROJECTS } from "./exactProjects";
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

// A project page lives in a project the person reads, and the Shell draws the not-found refusal
// for a project /projects does not list (frameFor, jc-multiorg-portal). A file's own fetch stub
// that answers that list without the project of the page under test (the window's address) gets
// it added: a person who opens a project page on prod reads that project. The file's mock still
// sees every call; a stub naming the project, answering no items, or marking its list
// exact (EXACT_PROJECTS, an outsider's on purpose) is kept as it is.

const stubGlobal = vi.stubGlobal.bind(vi);
vi.stubGlobal = (name: string | symbol | number, value: unknown) =>
  stubGlobal(name, name === "fetch" && typeof value === "function" ? listingThePage(value as typeof fetch) : value);

function listingThePage(stubbed: typeof fetch): typeof fetch {
  // A proxy, not a new function: a file reads its stub back through globalThis.fetch (`.mock.calls`,
  // `mockImplementation`, `.checks`), and every property is the file's own.
  return new Proxy(stubbed, {
    apply: (target, self, args: Parameters<typeof fetch>) => listing(Reflect.apply(target, self, args), ...args),
  });
}

async function listing(answer: Response | Promise<Response>, input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const response = await answer;
  const url = new URL(input instanceof Request ? input.url : String(input), window.location.href);
  const project = /^\/projects\/([^/]+)/.exec(window.location.pathname)?.[1];
  const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
  if (!project || project === "new" || method !== "GET" || url.pathname !== "/api/v1/projects" || !response?.ok || response.headers.has(EXACT_PROJECTS)) {
    return response;
  }
  const body = await response.clone().json().catch(() => null);
  if (!Array.isArray(body?.items) || body.items.some((item: { name?: string }) => item?.name === decodeURIComponent(project))) {
    return response;
  }
  return new Response(JSON.stringify({ ...body, items: [...body.items, { name: decodeURIComponent(project) }] }), {
    status: response.status,
    headers: response.headers,
  });
}
