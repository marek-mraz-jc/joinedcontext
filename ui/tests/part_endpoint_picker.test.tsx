/**
 * T-1798: the assistant's data bar against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * The bar had two hand-made buttons and a hand-made search box, each with its own focus ring and
 * its own size, and it took the focus with React's `autoFocus` prop on mount. They are the shared
 * `Button` and `Input` now, and the focus moves when the list opens, which is a click. What must
 * not change is what the bar is for: the endpoints a conversation may query, at most five, chosen
 * and removed by keyboard alone.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import {
  DataBar,
  MAX_ENDPOINTS,
  rememberEndpoints,
  storedEndpoints,
} from "../src/assistant/EndpointPicker";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const PROJECT = "helsinki";
const NAMES = ["helsinki-all", "helsinki-air", "helsinki-bikes", "helsinki-kpi", "helsinki-traffic", "helsinki-events"];

function endpoint(name: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name, project: PROJECT, title: { en: `${name} title`, sk: `${name} nazov` } },
    spec: { audience: name === "helsinki-kpi" ? "organization" : "public" },
  };
}

function stub(items = NAMES.map(endpoint)) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      ),
    ),
  );
}

function show(selected: string[] = []) {
  const onChange = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <DataBar project={PROJECT} selected={selected} onChange={onChange} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { onChange, user: userEvent.setup(), container: view.container };
}

const add = () => screen.getByRole("button", { name: i18n.t("assistant.data.add") });

describe("the assistant's data bar against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    sessionStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("has no axe violation closed, open, and with endpoints chosen", async () => {
    stub();
    const { container, user } = show(["helsinki-all"]);

    await expectNoViolations(container);

    await user.click(add());
    await screen.findByRole("option", { name: /helsinki-air/ });
    await expectNoViolations(container);
  });

  it("moves the focus into the search box when the list opens, and not before", async () => {
    stub();
    const { user } = show();

    // Nothing is focused on arrival: the bar sits above a text box a person is about to type in.
    expect(document.body).toHaveFocus();

    await user.click(add());
    const search = await screen.findByRole("combobox", { name: i18n.t("assistant.data.search") });
    await waitFor(() => expect(search).toHaveFocus());
    // The shared Input, so the ring, the hover and the disabled state come from one place.
    expect(search.className).toContain("focus-ring");
  });

  it("is chosen, filtered and closed by keyboard alone, and gives the focus back", async () => {
    stub();
    const { onChange, user } = show();

    add().focus();
    await user.keyboard("{Enter}");
    const search = await screen.findByRole("combobox", { name: i18n.t("assistant.data.search") });

    await user.type(search, "bikes");
    await waitFor(() =>
      expect(screen.getAllByRole("option").map((option) => option.textContent)).toHaveLength(1),
    );

    await user.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith(["helsinki-bikes"]);

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("option")).toBeNull();
    expect(add()).toHaveFocus();
  });

  it("walks the list with the arrow keys and says which option is the active one", async () => {
    stub();
    const { user } = show();

    await user.click(add());
    const search = await screen.findByRole("combobox", { name: i18n.t("assistant.data.search") });
    const first = (await screen.findAllByRole("option"))[0];
    expect(search).toHaveAttribute("aria-activedescendant", first.id);

    await user.keyboard("{ArrowDown}");
    expect(search).toHaveAttribute("aria-activedescendant", screen.getAllByRole("option")[1].id);
    await user.keyboard("{ArrowUp}");
    expect(search).toHaveAttribute("aria-activedescendant", first.id);
  });

  it("refuses a sixth endpoint in words, and says so on the options it refuses", async () => {
    stub();
    const chosen = NAMES.slice(0, MAX_ENDPOINTS);
    const { onChange, user } = show(chosen);

    await user.click(add());
    expect(
      await screen.findByText(i18n.t("assistant.data.full", { max: MAX_ENDPOINTS })),
    ).toBeInTheDocument();

    const sixth = screen.getByRole("option", { name: /helsinki-events/ });
    expect(sixth).toHaveAttribute("aria-disabled", "true");
    await user.click(sixth);
    expect(onChange).not.toHaveBeenCalled();

    // One already chosen can still be taken off, which is the way back under the limit.
    const chosenOption = screen.getByRole("option", { name: /helsinki-all/ });
    expect(chosenOption).toHaveAttribute("aria-selected", "true");
    await user.click(chosenOption);
    expect(onChange).toHaveBeenCalledWith(chosen.filter((name) => name !== "helsinki-all"));
  });

  it("removes a chosen endpoint by a button that names which one", async () => {
    stub();
    const { onChange, user } = show(["helsinki-all", "helsinki-air"]);

    const chosen = await screen.findByRole("list", { name: i18n.t("assistant.data.chosen") });
    const remove = within(chosen).getByRole("button", {
      name: i18n.t("assistant.data.remove", { name: "helsinki-air" }),
    });
    // The shared Button: 24 px is the smallest target WCAG 2.5.8 accepts, and the hand-made × was
    // smaller than that.
    expect(remove.className).toContain("h-control-xs");

    remove.focus();
    await user.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith(["helsinki-all"]);
  });

  it("keeps a long title readable and the full name reachable", async () => {
    const long = "helsinki-everything-the-city-publishes-about-air-quality-and-traffic";
    stub([endpoint(long)]);
    const { user } = show([long]);

    await user.click(add());
    // The pill truncates on a step of the scale; the whole name is in the title attribute and in
    // the list below, so nothing is only half-readable.
    const pill = screen.getByTitle(long);
    expect(pill.querySelector(".truncate")?.className).toContain("max-w-40");
    expect(await screen.findByRole("option", { name: new RegExp(long) })).toBeInTheDocument();
  });

  it("says nothing is chosen, and says when the project has no endpoint to choose", async () => {
    stub([]);
    const { user } = show();

    expect(await screen.findByText(i18n.t("assistant.data.empty"))).toBeInTheDocument();
    await user.click(add());
    expect(await screen.findByText(i18n.t("assistant.data.none"))).toBeInTheDocument();
  });

  it("keeps only the chosen names in the browser, and nothing else", () => {
    rememberEndpoints(PROJECT, ["helsinki-all", "helsinki-air"]);
    expect(storedEndpoints(PROJECT)).toEqual(["helsinki-all", "helsinki-air"]);

    // A preference (UI-09): names of endpoints, never a token, a draft or anything personal.
    const stored = Object.entries(sessionStorage).map(([key, value]) => `${key}=${String(value)}`);
    expect(stored.every((entry) => entry.startsWith("jc.assistant.endpoints."))).toBe(true);
    expect(stored.join("\n")).not.toMatch(/token|secret|password/i);

    // Anything else that ends up under the key is read as nothing, not as a crash.
    sessionStorage.setItem("jc.assistant.endpoints." + PROJECT, "{not json");
    expect(storedEndpoints(PROJECT)).toEqual([]);
    sessionStorage.setItem("jc.assistant.endpoints." + PROJECT, JSON.stringify([1, "a", null]));
    expect(storedEndpoints(PROJECT)).toEqual(["a"]);
  });

  it.each(SUPPORTED_LOCALES)("writes the bar in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    stub();
    const { container, user } = show();

    expect(screen.getByRole("group", { name: i18n.t("assistant.data.label") })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("assistant.data.add") }));
    await screen.findByRole("combobox", { name: i18n.t("assistant.data.search") });
    expectNoRawKeys(container);
  });
});
