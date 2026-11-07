/**
 * The ready-made set-up cards (T-1844, T-6096), against the UI contract (UI-15, UI-16, UI-11).
 *
 * The Flows page that listed every blueprint is gone (T-6096): the cards of one category sit on
 * the page that owns that work, the `ingestion` ones on Data sources. What this owns: the three
 * states of the list (waiting, refused, nothing to offer), the category filter, a card's heading
 * level under the section's, and every string drawn from a blueprint being text.
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { BlueprintCards } from "../src/pages/flows/Gallery";
import { expectNoRawKeys } from "./checks";
import { expectNoAxeViolations, inEveryLocale, json, list, problem, renderPage } from "./page_contract";

function blueprint(
  name: string,
  spec: Record<string, unknown> = {},
  metadata: Record<string, unknown> = {},
) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Blueprint",
    metadata: { name, title: { en: name }, description: { en: `What ${name} sets up` }, ...metadata },
    spec: { version: "1.2.0", category: "ingestion", ...spec },
  };
}

const GBFS = blueprint("gbfs-stations", { riskClass: "yellow" });
const MQTT = blueprint("mqtt-feed", { riskClass: "red" });
const APP = blueprint("app-from-prompt", { category: "application", riskClass: "yellow" });

function cards(
  blueprints: unknown[] | { fails: { status: number; detail: string } },
  onRun: (b: unknown) => void = () => {},
) {
  return renderPage(<BlueprintCards category="ingestion" onRun={onRun} />, {
    answer: (url) => {
      if (!url.pathname.endsWith("/blueprints")) return undefined;
      return Array.isArray(blueprints)
        ? json(list(blueprints))
        : problem(blueprints.fails.status, blueprints.fails.detail);
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the set-ups while there is nothing to show yet", () => {
  it("keeps its heading while the blueprints are being read, and says it is waiting", async () => {
    let answer!: () => void;
    const held = new Promise<void>((resolve) => {
      answer = resolve;
    });
    renderPage(<BlueprintCards category="ingestion" onRun={() => {}} />, {
      answer: async (url) => {
        if (!url.pathname.endsWith("/blueprints")) return undefined;
        await held;
        return json(list([GBFS]));
      },
    });

    expect(await screen.findByRole("heading", { level: 2, name: en.datasources.setups.title })).toBeInTheDocument();
    const waiting = await screen.findByRole("status");
    expect(waiting).toHaveAttribute("aria-busy", "true");

    answer();
    expect(await screen.findByRole("button", { name: en.flows.run })).toBeInTheDocument();
  });

  it("gives the server's own reason when the blueprints are unreachable, and a way to ask again", async () => {
    cards({ fails: { status: 503, detail: "The blueprint store is not answering." } });

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("The blueprint store is not answering.")).toBeInTheDocument();
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
  });

  // The page around it has its own way to add one; an empty section would only take room.
  it("draws nothing when the person may run no set-up of this category", async () => {
    const { container } = cards([APP]);

    await vi.waitFor(() => {
      expect(container.querySelector("[data-testid=blueprint-cards]")).toBeNull();
      expect(screen.queryByRole("status")).toBeNull();
    });
  });
});

describe("the cards", () => {
  it("draws one card per blueprint of the category, with its title, its lane and its version", async () => {
    cards([GBFS, MQTT, APP]);

    const items = await screen.findAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(within(items[0]).getByRole("heading", { level: 3, name: "gbfs-stations" })).toBeInTheDocument();
    expect(within(items[0]).getByText(en.flows.risk.yellow)).toBeInTheDocument();
    expect(within(items[1]).getByText(en.flows.risk.red)).toBeInTheDocument();
    expect(within(items[0]).getByText(en.flows.version.replace("{version}", "1.2.0"))).toBeInTheDocument();
    expect(screen.queryByText("app-from-prompt")).toBeNull();
  });

  it("hands the chosen blueprint to the page on Set up", async () => {
    const onRun = vi.fn();
    cards([GBFS], onRun);

    (await screen.findByRole("button", { name: en.flows.run })).click();
    expect(onRun).toHaveBeenCalledWith(expect.objectContaining({ metadata: expect.objectContaining({ name: "gbfs-stations" }) }));
  });
});

describe("what a blueprint says is data", () => {
  // CC-59: a blueprint is a manifest somebody else wrote. Its title and description are drawn.
  it("draws a title and a description that arrive as markup as text", async () => {
    const { container } = cards([
      blueprint("evil", {}, {
        title: { en: "<img src=x onerror=alert(1)>" },
        description: { en: "<script>alert(2)</script>" },
      }),
    ]);

    await screen.findAllByRole("listitem");
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(screen.getByRole("heading", { level: 3, name: "<img src=x onerror=alert(1)>" })).toBeInTheDocument();
  });

  it("draws no lane chip for a risk class it does not know", async () => {
    cards([blueprint("odd", { riskClass: "chartreuse" })]);

    const card = (await screen.findAllByRole("listitem"))[0];
    expect(within(card).queryByTitle(en.flows.riskLabel)).toBeNull();
  });
});

describe("the set-ups as a screen reader and a translator find them", () => {
  it("has no axe violations with cards on the screen", async () => {
    const { container } = cards([GBFS, MQTT]);

    await screen.findAllByRole("listitem");
    await expectNoAxeViolations(container);
  });

  it("draws every string of its own in all four locales", async () => {
    await inEveryLocale(async () => {
      const { container, unmount } = cards([GBFS, MQTT]);
      await screen.findAllByRole("listitem");
      expectNoRawKeys(container);
      expect(screen.getByRole("heading", { level: 2, name: i18n.t("datasources.setups.title") })).toBeInTheDocument();
      unmount();
    });
  });
});
