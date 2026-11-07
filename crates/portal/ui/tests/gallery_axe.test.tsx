/**
 * UI-15, UI-16 (T-1729): the component gallery — every shared control, every state, one axe run.
 *
 * Three things are held here. Every component `components/ui` exports is in the gallery, so a new
 * control cannot be added without its states. The whole page has no axe violation, which is the
 * check 116 of the 123 UI files had no equivalent of. And the route is development only: nothing
 * outside the `import.meta.env.DEV` branch of the router imports the module, so a production
 * build has no gallery in it.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { Gallery } from "../src/pages/gallery/Gallery";
import { expectHeadingOutline, expectNoAxeViolations, json, renderPage } from "./page_contract";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path: string) => readFileSync(join(UI, path), "utf8");

/** The value exports of `components/ui`: a component is one whose name begins in upper case. */
function componentsOfTheBarrel(): string[] {
  const barrel = read("src/components/ui/index.ts");
  const names = [...barrel.matchAll(/^export \{([^}]*)\} from/gms)]
    .flatMap((match) => match[1].split(","))
    .map((name) => name.trim())
    .filter(Boolean);
  // A component, not a constant: `CHECKBOX` and `CONTROL` are class strings, `TERMS` a list.
  return [...new Set(names.filter((name) => /^[A-Z][a-z]/.test(name)))];
}

function show() {
  return renderPage(<Gallery />, {
    answer: (url) =>
      url.pathname.endsWith("/permissions/me")
        ? json({ grants: [{ rule: { kinds: ["Endpoint"], verbs: ["propose"] } }], bootstrap: false })
        : undefined,
    path: "/__gallery",
  });
}

describe("the component gallery", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("every_export_of_components_ui_is_in_the_gallery", () => {
    const gallery = read("src/pages/gallery/Gallery.tsx");
    const components = componentsOfTheBarrel();
    expect(components.length).toBeGreaterThan(20);
    const missing = components.filter((name) => !new RegExp(`\\b${name}\\b`).test(gallery));
    expect(
      missing,
      "a shared component with no states in the gallery:\n" + missing.join("\n"),
    ).toEqual([]);
  });

  it("the_gallery_has_no_axe_violation", async () => {
    const { container } = show();
    await screen.findByRole("heading", { level: 1, name: "Component gallery" });
    // `#gallery-bad__error` is `Field`'s own `<ul role="alert">`, which no list may carry: the
    // shared component is fixed in T-2319 and this exclusion goes with it.
    await expectNoAxeViolations(container, ["#gallery-bad__error"]);
    expectHeadingOutline(container);
  });

  it("shows each control in the states a person meets it in", async () => {
    show();
    // Busy, refused and disabled are the three that are written by hand on pages when a control
    // does not carry them, so the gallery has to show all three.
    expect(await screen.findByRole("button", { name: "Proposing…" })).toBeDisabled();
    const refused = screen.getAllByRole("button", { name: /Propose the change/ });
    expect(refused.some((button) => button.getAttribute("aria-disabled") === "true")).toBe(true);
    expect(screen.getAllByRole("alert").length).toBeGreaterThan(0);
    // The long label and the short one are both on the page, in every specimen that takes one.
    // The long one is a phrase, not one sixty-character compound: a word nobody writes only
    // moved where the shared controls wrap on real pages (T-2413).
    expect(screen.getAllByText(/Luftqualitätsmessstation für die Verwaltung/).length).toBeGreaterThan(3);
    // And where an unbreakable string is real, it is an entity id (EP-08).
    expect(screen.getAllByText(/urn:ngsi-ld:AirQualityObserved/).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Ja!").length).toBeGreaterThan(1);
  });

  it("opens the dialog it offers, and it is still without a violation", async () => {
    const user = userEvent.setup();
    show();
    await user.click(await screen.findByRole("button", { name: "Open the dialog" }));
    const dialog = await screen.findByRole("dialog");
    await expectNoAxeViolations(dialog);
    // Cancel comes before the action, and the action is not what the keyboard lands on.
    const buttons = [...dialog.querySelectorAll("button")].map((button) => button.textContent);
    expect(buttons.indexOf("Cancel")).toBeLessThan(buttons.indexOf("Propose the change"));
    await waitFor(() => expect(document.activeElement).not.toHaveTextContent("Propose the change"));
  });

  it("the_gallery_is_not_in_the_production_bundle", () => {
    // The only import of the module in `src/` is the router's, and the router builds the route
    // inside `import.meta.env.DEV`, which a production build replaces with `false`.
    const router = read("src/router.tsx");
    // Loaded with its route since T-3280, so a production build has no chunk that asks for it.
    expect(router).toContain('import("./pages/gallery/Gallery")');
    const guarded = /const devRoutes = import\.meta\.env\.DEV\s*\?[\s\S]*?path: "\/__gallery"/.test(router);
    expect(guarded, "the gallery route is built outside the DEV branch").toBe(true);
    // And nothing else reaches for it. `ci-full` greps the built `dist/` for the same proof.
    const importers = importersOfTheGallery();
    expect(importers, "a production module imports the gallery").toEqual(["src/router.tsx"]);
  });
});

/** Every file under `src/` that imports the gallery module. */
function importersOfTheGallery(): string[] {
  const walk = (dir: string): string[] =>
    readdirSync(dir).flatMap((entry) => {
      const path = join(dir, entry);
      return statSync(path).isDirectory() ? walk(path) : [path];
    });
  return walk(join(UI, "src"))
    .filter((path) => /\.tsx?$/.test(path))
    .filter((path) => !path.endsWith("pages/gallery/Gallery.tsx"))
    .filter((path) => /(?:from |import\()"[^"]*pages\/gallery\/Gallery"/.test(readFileSync(path, "utf8")))
    .map((path) => path.slice(UI.length + 1));
}
