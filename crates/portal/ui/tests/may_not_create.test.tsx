/**
 * The reason a viewer cannot create, on screen (T-4263, UI-44, PF-50): the guard's reason was a
 * tooltip only, so a row of greyed create buttons, an enabled "Import rows", a live upload form and
 * a red "no create form" warning met a read-only role with nothing it could read on why.
 */
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { proposesAnything } from "../src/api/permissions";
import type { Effective } from "../src/api/permissions";
import { renderRoute } from "./pageHarness";

const VIEWER = { project: "helsinki", bootstrap: false, grants: [] };
const SPACE_EDITOR = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "space-editor", binding: "editors", rule: { kinds: ["ContextSpace"], verbs: ["propose"] } }],
};

afterEach(() => {
  vi.unstubAllGlobals();
  window.history.pushState({}, "", "/");
});

// The line under a list's header went away (owner 2026-11-01, T-6344): the greyed control keeps
// its reason, on the control and for a screen reader.
describe("a list's greyed create button", () => {
  it("says on the control why a viewer may not create, with no line under the header", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces", permissions: VIEWER });
    const add = await screen.findByRole("button", { name: new RegExp(en.spaces.add) });
    await waitFor(() => expect(add).toHaveAttribute("aria-disabled", "true"));
    expect(add).toHaveAccessibleDescription(/ContextSpace/);
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
    expect(screen.getAllByRole("button", { name: new RegExp(en.spaces.fromFile) })[0]).toHaveAttribute("aria-disabled", "true");
  });

  it("is enabled for a role that may create there", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces", permissions: SPACE_EDITOR });
    const add = await screen.findByRole("button", { name: new RegExp(en.spaces.add) });
    await waitFor(() => expect(add).not.toHaveAttribute("aria-disabled", "true"));
  });

  it("leads from the list to the file upload (T-6450)", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces", permissions: SPACE_EDITOR });
    const [fromFile] = await screen.findAllByRole("button", { name: new RegExp(en.spaces.fromFile) });
    await waitFor(() => expect(fromFile).not.toHaveAttribute("aria-disabled", "true"));
    fireEvent.click(fromFile);
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/spaces/upload"));
  });
});

describe("the space pages that create for a viewer", () => {
  it("shows the reason on the completion page in place of its inputs", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/complete", permissions: VIEWER });
    expect((await screen.findByText(en.form.mayNotCreate)).closest("[role=status]")).not.toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button", { name: en.spaces.complete.title })).toBeNull();
  });

  it("shows the reason on the upload page in place of its file picker", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/upload", permissions: VIEWER });
    expect((await screen.findByText(en.form.mayNotCreate)).closest("[role=status]")).not.toBeNull();
    expect(screen.queryByText(en.spaces.upload.chooseButton)).toBeNull();
  });

  it("keeps the upload page for a role that may propose a space", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/upload", permissions: SPACE_EDITOR });
    expect(await screen.findByText(en.spaces.upload.chooseButton)).toBeInTheDocument();
    expect(screen.queryByText(en.form.mayNotCreate)).toBeNull();
  });
});

describe("proposesAnything", () => {
  const doc = (grants: unknown[], bootstrap = false) => ({ project: "helsinki", bootstrap, grants }) as unknown as Effective;

  it("is false for a role with no propose grant", () => {
    expect(proposesAnything(doc([]))).toBe(false);
    expect(proposesAnything(doc([{ rule: { kinds: ["Endpoint"], verbs: ["read"] } }]))).toBe(false);
  });

  it("is true for one propose grant, for a bootstrap administrator and with no document", () => {
    expect(proposesAnything(doc([{ rule: { kinds: ["App"], verbs: ["read", "propose"] } }]))).toBe(true);
    expect(proposesAnything(doc([], true))).toBe(true);
    expect(proposesAnything(undefined)).toBe(true);
  });
});
