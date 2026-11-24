/**
 * The Context Spaces list's remaining branches: the row menu's Open, the upload door, searching
 * and sorting on the grid's text columns, and the history column's states.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";

const PATH = "/projects/helsinki/spaces";

const space = (name: string, over: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name, namespace: "helsinki", ...(over.metadata as object) },
  spec: { dataModelRef: "AirQualityObserved", ...(over.spec as object) },
  status: { phase: "Live" },
});

type Usage = Record<string, unknown> | Response;

function answering(items: unknown[], usage: Record<string, Usage> = {}) {
  return (path: string): Response | undefined => {
    const hit = /\/spaces\/([a-z-]+)\/usage$/.exec(path);
    if (hit) {
      const own = usage[hit[1]];
      if (own instanceof Response) return own;
      return jsonResponse({ entities: 1, broker: "main", observedAt: "2026-09-25T11:40:00Z", ...own });
    }
    if (path.endsWith("/spaces")) return jsonResponse(list(items));
    return undefined;
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("spaces list, remaining branches", () => {
  it("opens a space from the first entry of its row menu", async () => {
    await renderRoute({ path: PATH, answer: answering([space("air")]) });
    const table = await screen.findByRole("table");
    const user = userEvent.setup();
    await user.click(await within(table).findByRole("button", { name: /More actions for/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceList.open }));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/spaces/air"));
  });

  it("goes to the upload page from the From a file button", async () => {
    await renderRoute({ path: PATH, answer: answering([space("air")]) });
    const button = await screen.findByRole("button", { name: en.spaces.fromFile });
    await waitFor(() => expect(button).toBeEnabled());
    await userEvent.click(button);
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/spaces/upload"));
  });

  it("finds a space by its title or its name", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([
        space("air", { metadata: { title: { en: "Air quality" } } }),
        space("bikes", { spec: { dataModelRef: "Bicycle" } }),
      ]),
    });
    const table = await screen.findByRole("table");
    const rows = () => within(table).getAllByRole("row").length - 1;
    await waitFor(() => expect(rows()).toBe(2));
    const user = userEvent.setup();
    const box = screen.getByRole("searchbox");
    await user.type(box, "quality");
    await waitFor(() => expect(rows()).toBe(1));
    expect(within(table).getByText("Air quality")).toBeInTheDocument();
    await user.clear(box);
    await user.type(box, "bikes");
    await waitFor(() => expect(rows()).toBe(1));
    expect(within(table).queryByText("Air quality")).toBeNull();
    await user.clear(box);
    await user.type(box, "nothing-like-this");
    expect(await screen.findByText(en.grid.noMatch)).toBeInTheDocument();
    expect(within(table).queryByText("bikes")).toBeNull();
  });

  it("filters by lifecycle phase", async () => {
    const waiting = { ...space("bikes"), status: { phase: "Pending" } };
    await renderRoute({ path: PATH, answer: answering([space("air"), waiting]) });
    const table = await screen.findByRole("table");
    const user = userEvent.setup();
    await user.click(screen.getByText(en.grid.filter));
    const boxes = screen.getAllByRole("checkbox");
    expect(boxes.length).toBeGreaterThanOrEqual(2);
    await user.click(screen.getByRole("checkbox", { name: /Pending/ }));
    expect(within(table).getAllByRole("row")).toHaveLength(2);
    expect(within(table).getByText("bikes")).toBeInTheDocument();
  });

  it("sorts by the history count and shows an estimated count, a missing count and a size", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([space("air"), space("bikes"), space("noise")], {
        air: { attrInstances: 5000, estimated: true, bytes: { attrInstances: 2048 } },
        bikes: { attrInstances: 12 },
        noise: {},
      }),
    });
    const table = await screen.findByRole("table");
    await waitFor(() => expect(within(table).getAllByTestId("space-history")).toHaveLength(3));
    const history = () => within(table).getAllByTestId("space-history").map((cell) => cell.textContent);
    expect(history()).toEqual(["≈ 5,000", "12", "—"]);
    expect(within(table).getAllByTestId("space-history")[0]).toHaveAttribute("title", en.spaces.historyEstimated);
    expect(within(table).getAllByTestId("space-history-size")[0]).toHaveTextContent("2 KiB");
    const sort = within(table).getByRole("button", { name: en.spaces.field.history });
    await userEvent.click(sort);
    expect(history()).toEqual(["12", "≈ 5,000", "—"]);
    await userEvent.click(sort);
    expect(history()).toEqual(["≈ 5,000", "12", "—"]);
  });

  it("says a count is not known while it is still on its way, and keeps the history column quiet", async () => {
    await renderRoute({
      path: PATH,
      answer: (path, request) =>
        path.endsWith("/usage")
          ? new Promise<Response>((_, reject) => request.signal.addEventListener("abort", () => reject(request.signal.reason)))
          : answering([space("air")])(path),
    });
    const table = await screen.findByRole("table");
    expect(await within(table).findByText(en.spaces.entitiesCounting)).toBeInTheDocument();
    expect(within(table).queryByTestId("space-history")).toBeNull();
  });

  it("gives the refusal as the reason a count is missing, in the tooltip", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([space("air")], { air: problem(403, "You may not count this space.") }),
    });
    const table = await screen.findByRole("table");
    const reason = i18n.t("spaces.entitiesUnknown", { reason: "You may not count this space." });
    const text = await within(table).findByText(reason);
    expect(text.closest("[title]")).toHaveAttribute("title", reason);
  });

  it("sorts by the entity count and keeps a space with no count last either way", async () => {
    await renderRoute({
      path: PATH,
      answer: answering([space("air"), space("bikes"), space("noise")], {
        air: { entities: 30 },
        bikes: problem(503, "not now"),
        noise: { entities: 4 },
      }),
    });
    const table = await screen.findByRole("table");
    await waitFor(() => expect(within(table).getAllByTestId("space-entities")).toHaveLength(2));
    const sort = within(table).getByRole("button", { name: en.spaces.field.entities });
    const order = () =>
      within(table)
        .getAllByRole("row")
        .slice(1)
        .map((row) => within(row).getAllByRole("link")[0].textContent);
    await userEvent.click(sort);
    expect(order()).toEqual(["noise", "air", "bikes"]);
    await userEvent.click(sort);
    expect(order()).toEqual(["air", "noise", "bikes"]);
  });
});
