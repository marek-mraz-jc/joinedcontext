/** Helsinki's open data as grids over what the public endpoint answers (T-2788). */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { DATASETS } from "./datasets";
import { answer, byId } from "./fixtures/helsinki";
import { LOCALES } from "./locales";

const s = LOCALES.fi;
const SLUG = "mc4hz7tkq2vbr6xna3wjd5yep2";
let asked: URL[];

function show(withEndpoint = true, endpoints?: { name: string; slug: string; space: string; types: string[] }[]) {
  asked = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      asked.push(url);
      // The entity panel reads the one entity it shows.
      const one = /\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
      if (one) {
        const entity = byId(decodeURIComponent(one[1]));
        return new Response(JSON.stringify(entity ?? { title: "Not found" }), { status: entity ? 200 : 404, headers: { "content-type": "application/json" } });
      }
      const rows = answer(url.searchParams.get("type"));
      return new Response(JSON.stringify(rows), {
        status: 200,
        headers: { "content-type": "application/json", "NGSILD-Results-Count": String(rows.length) },
      });
    }),
  );
  const client = stubClient(undefined, {
    slug: withEndpoint ? SLUG : "",
    orgDomain: "hel.fi",
    space: withEndpoint ? "helsinki" : "elsewhere",
    transport: "origin",
    appName: "helsinki-data",
    language: "fi",
    ...(endpoints ? { endpoints } : {}),
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("Helsinki's datasets", () => {
  it("opens on the services, read from the endpoint by type, with the city's column labels", async () => {
    show();
    expect(screen.getByRole("tab", { name: s.dataset.services })).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByRole("tabpanel", { name: s.dataset.services });
    await waitFor(() => expect(within(panel).getByText("Keskustakirjasto Oodi")).toBeInTheDocument());
    expect(within(panel).getByRole("columnheader", { name: new RegExp(s.column.serviceCategory) })).toBeInTheDocument();
    expect(asked.some((url) => url.pathname.includes(`/api/endpoint/${SLUG}/`) && url.searchParams.get("type") === "PointOfInterest")).toBe(true);
  });

  it("moves between datasets with the arrow keys and shows each one's grid and downloads", async () => {
    show();
    screen.getByRole("tab", { name: s.dataset.services }).focus();
    await userEvent.keyboard("{ArrowRight}");
    const permits = screen.getByRole("tab", { name: s.dataset.permits });
    expect(permits).toHaveFocus();
    const panel = screen.getByRole("tabpanel", { name: s.dataset.permits });
    await waitFor(() => expect(within(panel).getByText("Kaukolämpöputken korjaus")).toBeInTheDocument());
    expect(within(panel).getByRole("link", { name: `${s.csv} ${s.dataset.permits}` })).toHaveAttribute(
      "href",
      `/api/endpoint/${SLUG}/file.csv?type=CityWork&humanHeaders=true`,
    );
    expect(panel.querySelectorAll(".downloads a"), "one download: the gateway serves no GeoJSON file (T-4597, T-4603)").toHaveLength(1);
    await userEvent.keyboard("{End}");
    expect(screen.getByRole("tab", { name: s.dataset.weather })).toHaveFocus();
  });

  it("reads through the endpoint of the city's space where the App is served several", async () => {
    show(false, [
      { name: "app-other", slug: "otherslug", space: "elsewhere", types: ["Thing"] },
      { name: "app-helsinki-data", slug: SLUG, space: "helsinki", types: ["PointOfInterest"] },
    ]);
    await waitFor(() => expect(screen.getByText("Keskustakirjasto Oodi")).toBeInTheDocument());
    expect(asked.every((url) => url.pathname.includes(`/api/endpoint/${SLUG}/`))).toBe(true);
  });

  it("says it has nothing to read when the app has no endpoint of the city's space", () => {
    show(false);
    expect(screen.getByRole("status")).toHaveTextContent(s.noEndpoint);
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("names its source and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByText("Keskustakirjasto Oodi")).toBeInTheDocument());
    expect(screen.getByText(s.source)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });

  // T-3373: every control the App shows answers, for every dataset: each column sorts, shows its
  // details, and takes each kind of filter the endpoint asks with its own `q`; a row opens in the
  // shell's entity panel (SDK-40) and closes again; the two downloads are offered. Each round
  // works the controls not yet worked, since a chosen filter brings its value fields.
  it("answers every control of every dataset's grid, a row in the entity panel, without an error", async () => {
    const errors = vi.spyOn(console, "error");
    show();
    const user = userEvent.setup();
    // A download is the browser's; the test stops it before jsdom would navigate.
    const stop = (event: Event) => event.preventDefault();
    document.addEventListener("click", stop);
    for (const dataset of DATASETS) {
      await user.click(screen.getByRole("tab", { name: s.dataset[dataset] }));
      const panel = screen.getByRole("tabpanel", { name: s.dataset[dataset] });
      await waitFor(() => expect(within(panel).getAllByRole("row").length).toBeGreaterThan(2));
      // One control at a time, the page read again after each: what one opens (a column's
      // details, the query as text) is worked before the next control closes it.
      const worked = new Set<string>();
      for (let step = 0; step < 400; step++) {
        const next = ROLES.flatMap((role) => within(panel).queryAllByRole(role))
          .sort((a, b) => (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1))
          .find((control) => !worked.has(keyOf(control)));
        if (!next) break;
        worked.add(keyOf(next));
        await work(user, next);
      }
      expect(asked.some((url) => url.searchParams.has("q"))).toBe(true);
    }
    document.removeEventListener("click", stop);
    expect(errors).not.toHaveBeenCalled();
  }, 120_000);

  it("wraps the tabs round with the arrow keys and leaves other keys to the page", async () => {
    show();
    screen.getByRole("tab", { name: s.dataset.services }).focus();
    await userEvent.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: s.dataset.weather })).toHaveFocus();
    await userEvent.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: s.dataset.services })).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByRole("tab", { name: s.dataset.services })).toHaveAttribute("aria-selected", "true");
  });
});

const ROLES = ["button", "combobox", "listbox", "checkbox", "link", "textbox", "spinbutton"] as const;

/** A control by its role and name, the way the coverage gate tells one from another. */
function keyOf(control: HTMLElement): string {
  const label = control instanceof HTMLInputElement ? control.labels?.[0]?.textContent : null;
  return `${control.getAttribute("role") ?? control.tagName}:${control.getAttribute("type") ?? ""}:${control.getAttribute("aria-label") ?? label ?? control.textContent}`;
}

/** Works one control as a person would: a click, a choice, or a value typed. */
async function work(user: ReturnType<typeof userEvent.setup>, control: HTMLElement) {
  if (control instanceof HTMLSelectElement) {
    const choices = Array.from(control.options).filter((option) => option.value !== "");
    if (choices.length === 0) return;
    if (control.multiple) await user.selectOptions(control, choices[0]);
    else await user.selectOptions(control, choices[choices.length - 1]);
    return;
  }
  if (control instanceof HTMLInputElement && control.type !== "checkbox") {
    const typed = control.type === "date" ? "2026-09-01" : control.type === "number" ? "1" : "a";
    fireEvent.change(control, { target: { value: typed } });
    return;
  }
  await user.click(control);
  // A row opened in the shell's panel is read, then closed, so the next control is on the page.
  // A column's menu is a dialog too, with no close button: it closes on Escape.
  const dialog = screen.queryByRole("dialog");
  if (dialog?.classList.contains("jc-grid-col-menu")) fireEvent.keyDown(dialog, { key: "Escape" });
  else if (dialog) await user.click(await within(dialog).findByRole("button", { name: s.grid.close }));
}