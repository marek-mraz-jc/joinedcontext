/**
 * T-7585: the per-class editors of an Endpoint's calendar, feed and map outputs write what a
 * person picks: the class, the starting preset, the attribute of a field or role, the map's
 * geometry.
 */
import { useState } from "react";
import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { FeedSourceEditor, IcalSourceEditor, MapSourceEditor } from "../src/components/forms/widgets/OutputSourceEditors";
import type { IcalMapping } from "../src/components/forms/widgets/icalMapping";
import type { FeedSource, MapSource } from "../src/components/forms/widgets/representationOutputs";

function wrap(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

function Held<S>({ initial, seen, editor: Editor }: { initial: S; seen: (next: S) => void; editor: (props: EditorProps<S>) => ReactNode }) {
  const [value, setValue] = useState(initial);
  return (
    <Editor
      id="out"
      value={value}
      project="helsinki"
      space=""
      model={undefined}
      frozen={false}
      onChange={(next: S) => {
        seen(next);
        setValue(next);
      }}
    />
  );
}

interface EditorProps<S> {
  id: string;
  value: S;
  onChange: (next: S) => void;
  project: string;
  space: string;
  model: undefined;
  frozen: boolean;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
      if (url.pathname === "/api/v1/organization/datamodels") {
        return Response.json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [{ name: "mobility", level: "project", project: "helsinki", space: "mobility", version: "0.3.0", lifecycle: "draft", classes: ["Road"] }],
          smartDataModels: [],
        });
      }
      return new Response("not found", { status: 404 });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function pickRoad() {
  await userEvent.click(document.querySelector("#out-type") as HTMLInputElement);
  await userEvent.click(await screen.findByRole("option", { name: /Road/ }));
}

describe("the calendar output's class", () => {
  it("writes the class picked, the preset it starts from and an attribute for a field", async () => {
    const seen: IcalMapping[] = [];
    const { container, unmount } = wrap(<Held<IcalMapping> initial={{ type: "Event" }} seen={(next) => seen.push(next)} editor={IcalSourceEditor} />);
    await userEvent.selectOptions(container.querySelector("#out-preset") as HTMLSelectElement, "schema-org");
    expect(seen.at(-1)).toMatchObject({ type: "Event", preset: "schema-org" });
    await userEvent.selectOptions(screen.getByLabelText("SUMMARY"), "location");
    expect(seen.at(-1)?.mapping).toMatchObject({ SUMMARY: "location" });
    await pickRoad();
    expect(seen.at(-1)?.type).toBe("Road");
    unmount();
  });
});

describe("the feed output's class", () => {
  it("writes the class picked", async () => {
    const seen: FeedSource[] = [];
    wrap(<Held<FeedSource> initial={{ type: "Event" }} seen={(next) => seen.push(next)} editor={FeedSourceEditor} />);
    await pickRoad();
    expect(seen.at(-1)).toMatchObject({ type: "Road" });
  });
});

describe("the map output's class and geometry", () => {
  it("writes the class picked, and the geometry attribute chosen", async () => {
    const seen: MapSource[] = [];
    const { container } = wrap(<Held<MapSource> initial={{ type: "Event" }} seen={(next) => seen.push(next)} editor={MapSourceEditor} />);
    const geometry = container.querySelector("#out-geometry") as HTMLSelectElement;
    const offered = [...geometry.options].map((option) => option.value).filter(Boolean);
    expect(offered.length).toBeGreaterThan(0);
    await userEvent.selectOptions(geometry, offered[0]);
    expect(seen.at(-1)?.mapping?.geometry).toBe(offered[0]);
    await pickRoad();
    expect(seen.at(-1)).toEqual({ type: "Road" });
  });
});
