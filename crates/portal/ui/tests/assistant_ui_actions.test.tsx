/**
 * T-5959: the assistant's browser actions on the open page. A highlight points at the field and
 * says why as text, a filter types into the page's own search box, a missing target is reported
 * back, and a manifest the assistant opened the create form with is read into the form. None of
 * them submits anything.
 */
import { render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { runUiAction, uiActionOf } from "../src/assistant/uiActions";
import { prefilledForm } from "../src/routes/PipelinesPage";

afterEach(() => {
  document.body.innerHTML = "";
});

function Filter({ onSubmit }: { onSubmit: () => void }) {
  const [needle, setNeedle] = useState("");
  return (
    <form onSubmit={onSubmit}>
      <input type="search" aria-label="Find" value={needle} onChange={(event) => setNeedle(event.target.value)} />
      <p>showing {needle || "all"}</p>
    </form>
  );
}

describe("assistant browser actions", () => {
  it("reads only the actions it knows", () => {
    expect(uiActionOf({ action: "highlight", field: "spec.compute", message: "here" })).toEqual({
      action: "highlight",
      field: "spec.compute",
      message: "here",
    });
    expect(uiActionOf({ action: "filter", query: "news" })).toEqual({ action: "filter", query: "news" });
    expect(uiActionOf({ action: "submit" })).toBeNull();
    expect(uiActionOf({ action: "highlight", field: 3, message: "x" })).toBeNull();
  });

  it("highlights a field with its message as text, never markup", async () => {
    render(<input id="root_compute_bloblang" aria-label="Mapping" />);
    const acted = await runUiAction(
      { action: "highlight", field: "spec.compute.bloblang", message: "<img src=x onerror=alert(1)> check" },
      0,
    );
    expect(acted).toBe(true);
    const control = screen.getByLabelText("Mapping");
    const note = screen.getByRole("status");
    expect(note.textContent).toBe("<img src=x onerror=alert(1)> check");
    expect(note.querySelector("img")).toBeNull();
    expect(control).toHaveFocus();
    expect(control.getAttribute("aria-describedby")).toContain(note.id);
    expect(control.hasAttribute("data-assistant-highlight")).toBe(true);
  });

  it("moves the note: the field pointed at before is described by it no more, nor twice (T-6230)", async () => {
    render(
      <>
        <input id="root_name" aria-label="Name" aria-describedby="name-hint" />
        <input id="root_title" aria-label="Title" />
      </>,
    );
    await runUiAction({ action: "highlight", field: "spec.name", message: "one" }, 0);
    await runUiAction({ action: "highlight", field: "spec.name", message: "again" }, 0);
    const note = screen.getByRole("status");
    expect(screen.getByLabelText("Name").getAttribute("aria-describedby")).toBe(`name-hint ${note.id}`);
    await runUiAction({ action: "highlight", field: "spec.title", message: "two" }, 0);
    expect(screen.getByLabelText("Name").getAttribute("aria-describedby")).toBe("name-hint");
    expect(screen.getByLabelText("Title").getAttribute("aria-describedby")).toBe(screen.getByRole("status").id);
  });

  it("types a filter into the page's search box without submitting", async () => {
    const onSubmit = vi.fn();
    render(<Filter onSubmit={onSubmit} />);
    expect(await runUiAction({ action: "filter", query: "news" }, 0)).toBe(true);
    expect(await screen.findByText("showing news")).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("reports a page without the field or the filter", async () => {
    render(<p>nothing here</p>);
    expect(await runUiAction({ action: "highlight", field: "spec.nope", message: "x" }, 0)).toBe(false);
    expect(await runUiAction({ action: "filter", query: "x" }, 0)).toBe(false);
  });

  it("waits for a field the opened page mounts after the action arrived", async () => {
    const pending = runUiAction({ action: "highlight", field: "spec.title", message: "name it" }, 2000);
    window.setTimeout(() => {
      const late = document.createElement("input");
      late.id = "root_title";
      document.body.append(late);
    }, 150);
    expect(await pending).toBe(true);
  });

  it("reads a Pipeline manifest the assistant opened the create form with", () => {
    const form = prefilledForm({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "hel-news", namespace: "helsinki" },
      spec: { compute: { kind: "bloblang", bloblang: "root = this" } },
    });
    expect(form?.name).toBe("hel-news");
    expect(form?.compute?.bloblang).toBe("root = this");
    expect(prefilledForm(null)).toBeUndefined();
    expect(prefilledForm({ name: "plain-form" })?.name).toBe("plain-form");
  });
});
