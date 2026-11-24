/**
 * The pickers' failure and edge branches: a list the server refused says why and offers a retry
 * that asks again, a multi-select toggles a chosen value off, and typed text that names a known
 * option picks it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { Combobox } from "../src/components/pickers/Combobox";
import { TypePicker } from "../src/components/pickers/TypePicker";
import { AttributePicker } from "../src/components/pickers/AttributePicker";
import { PersonPicker, idsOf } from "../src/components/pickers/PersonPicker";
import { ResourceNamePicker } from "../src/components/pickers/ResourceNamePicker";
import { EntitySearchPicker } from "../src/components/pickers/EntitySearchPicker";

const problem = (status: number, detail: string) =>
  new Response(JSON.stringify({ title: "Failed", status, detail }), {
    status,
    headers: { "Content-Type": "application/problem+json" },
  });
const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });

let paths: string[] = [];
let handler: (url: URL) => Response;

function wrap(node: ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  paths = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
      paths.push(url.pathname);
      return handler(url);
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

const count = (path: string) => paths.filter((p) => p === path).length;

const MODEL = {
  name: "mobility",
  level: "project",
  project: "helsinki",
  space: "mobility",
  version: "1.0.0",
  lifecycle: "published",
  classes: ["Road"],
};
const SHARED = { name: "shared", level: "project", project: "helsinki", version: "1.0.0", lifecycle: "published", classes: ["Kiosk"] };
const LIST = "/api/v1/organization/datamodels";
const SOURCE = "/api/v1/projects/helsinki/datamodels/mobility/source";

describe("Combobox", () => {
  const OPTIONS = [
    { value: "road", label: "road", group: "Parts" },
    { value: "bridge", label: "bridge", group: "Parts" },
  ];

  it("takes a chosen value off again when it is picked a second time in a multiple list", async () => {
    const onChange = vi.fn();
    wrap(<Combobox label="Parts" multiple value={["road", "bridge"]} options={OPTIONS} empty="none" onChange={onChange} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Parts" }));
    await userEvent.click(screen.getByRole("option", { name: /bridge/ }));
    expect(onChange).toHaveBeenCalledWith(["road"]);
  });

  it("picks the known option when the typed text only differs in case and the person leaves", async () => {
    const onChange = vi.fn();
    const onCreate = vi.fn();
    wrap(
      <>
        <Combobox label="Parts" value={[]} options={OPTIONS} empty="none" onChange={onChange} create={{ label: (n) => `Create ${n}`, onCreate }} />
        <button type="button">elsewhere</button>
      </>,
    );
    await userEvent.type(screen.getByRole("combobox", { name: "Parts" }), "ROAD");
    await userEvent.click(screen.getByRole("button", { name: "elsewhere" }));
    expect(onChange).toHaveBeenCalledWith(["road"]);
    expect(onCreate).not.toHaveBeenCalled();
  });

  it("moves the active option up with ArrowUp and not past the first", async () => {
    const onChange = vi.fn();
    wrap(<Combobox label="Parts" value={[]} options={OPTIONS} empty="none" onChange={onChange} />);
    const box = screen.getByRole("combobox", { name: "Parts" });
    await userEvent.click(box);
    await userEvent.keyboard("{ArrowDown}{ArrowUp}{ArrowUp}{Enter}");
    expect(onChange).toHaveBeenCalledWith(["road"]);
  });
});

describe("TypePicker failures", () => {
  it("says why the model list failed and asks again on retry", async () => {
    handler = () => problem(500, "the mirror is not ready");
    wrap(<TypePicker label="Type" project="helsinki" value={[]} onChange={() => {}} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Type" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("the mirror is not ready");
    const before = count(LIST);
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(count(LIST)).toBeGreaterThan(before));
  });

  it("says why a model source failed and asks for it again on retry", async () => {
    handler = (url) => (url.pathname === LIST ? json({ items: [MODEL], smartDataModels: [] }) : problem(500, "source unreadable"));
    wrap(<TypePicker label="Type" project="helsinki" space="mobility" value={[]} onChange={() => {}} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Type" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("source unreadable");
    const before = count(SOURCE);
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(count(SOURCE)).toBeGreaterThan(before));
  });

  it("offers a project model the space imports by its project level, and creates a new type", async () => {
    handler = (url) =>
      url.pathname === LIST
        ? json({ items: [MODEL, SHARED], smartDataModels: [] })
        : new Response("imports:\n  - project.shared.v1\nclasses: {}\n", { headers: { "Content-Type": "text/yaml" } });
    const onCreate = vi.fn();
    wrap(<TypePicker label="Type" project="helsinki" space="mobility" value={[]} onChange={() => {}} onCreate={onCreate} />);
    await userEvent.type(screen.getByRole("combobox", { name: "Type" }), "Lamp");
    await userEvent.click(await screen.findByRole("option", { name: /Lamp/ }));
    expect(onCreate).toHaveBeenCalledWith("Lamp");
    await userEvent.clear(screen.getByRole("combobox", { name: "Type" }));
    await userEvent.click(screen.getByRole("combobox", { name: "Type" }));
    await waitFor(() => expect(screen.getByRole("listbox").textContent).toContain("Imported from helsinki/shared"));
    expect(screen.getByRole("listbox").textContent).toContain("Kiosk");
  });
});

describe("AttributePicker failures", () => {
  it("retries the model list and then a model source", async () => {
    handler = () => problem(500, "no list");
    const first = wrap(<AttributePicker label="Attribute" project="helsinki" type="Road" value={[]} onChange={() => {}} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Attribute" }));
    let alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no list");
    const before = count(LIST);
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(count(LIST)).toBeGreaterThan(before));
    first.unmount();

    handler = (url) => (url.pathname === LIST ? json({ items: [MODEL], smartDataModels: [] }) : problem(500, "no source"));
    wrap(<AttributePicker label="Attribute" project="helsinki" type="Road" value={[]} onChange={() => {}} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Attribute" }));
    alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no source");
    const sourceBefore = count(SOURCE);
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(count(SOURCE)).toBeGreaterThan(sourceBefore));
  });
});

describe("PersonPicker failures", () => {
  const readsPeople = { project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Person"], verbs: ["read"] } }] };

  it("says the people list failed and asks again on retry", async () => {
    handler = (url) => (url.pathname.endsWith("/permissions/me") ? json(readsPeople) : problem(500, "directory down"));
    function Holder() {
      const [value, setValue] = useState("");
      return <PersonPicker label="Person" value={value} onChange={setValue} />;
    }
    wrap(<Holder />);
    await userEvent.click(screen.getByRole("combobox", { name: "Person" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("directory down");
    const before = count("/api/v1/organization/people");
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(count("/api/v1/organization/people")).toBeGreaterThan(before));
  });

  it("treats an address whose lookup fails as naming nobody", async () => {
    handler = () => problem(500, "directory down");
    const answer = await idsOf("jana@hel.fi", true);
    expect(answer.ids).toEqual([]);
    expect(answer.refused).toEqual(["jana@hel.fi"]);
  });
});

describe("ResourceNamePicker failures", () => {
  it("retries a failed project list and a failed manifest list", async () => {
    handler = () => problem(500, "projects unreadable");
    const first = wrap(<ResourceNamePicker label="Project" from="projects" value="" onChange={() => {}} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Project" }));
    let alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("projects unreadable");
    const before = paths.length;
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(paths.length).toBeGreaterThan(before));
    first.unmount();

    paths = [];
    handler = () => problem(500, "roles unreadable");
    wrap(<ResourceNamePicker label="Role" from={{ project: "helsinki", plural: "roles" }} value="" onChange={() => {}} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Role" }));
    alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("roles unreadable");
    const rolesBefore = paths.length;
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(paths.length).toBeGreaterThan(rolesBefore));
  });

  it("offers the typed text as a new name and reports it", async () => {
    handler = () => json({ items: [] });
    const onChange = vi.fn();
    wrap(<ResourceNamePicker label="Role" create from={{ project: "helsinki", plural: "roles" }} value="" onChange={onChange} />);
    await userEvent.type(screen.getByRole("combobox", { name: "Role" }), "editor");
    await userEvent.click(await screen.findByRole("option", { name: /editor/ }));
    expect(onChange).toHaveBeenCalledWith("editor");
  });
});

describe("EntitySearchPicker failures", () => {
  it("says the search failed and searches again on retry", async () => {
    handler = () => problem(500, "gateway down");
    wrap(<EntitySearchPicker label="Station" space="air" type="Station" value="" onChange={() => {}} />);
    await userEvent.click(screen.getByRole("combobox", { name: "Station" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("gateway down");
    const before = paths.length;
    await userEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(paths.length).toBeGreaterThan(before));
  });
});
