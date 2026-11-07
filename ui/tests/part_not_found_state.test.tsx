/**
 * T-2749: the not-found state a section the API does not serve shares with the router's fallback.
 * The address is the one thing on it a stranger chose, so it is shown as text, never as markup.
 */
import { render, screen } from "@testing-library/react";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { NotFoundState } from "../src/components/NotFoundState";
import { expectNoRawKeys, expectNoViolations } from "./checks";

function show(path: string) {
  const root = createRootRoute();
  const page = createRoute({ getParentRoute: () => root, path: "$", component: NotFoundState });
  const router = createRouter({
    routeTree: root.addChildren([page]),
    history: createMemoryHistory({ initialEntries: [path] }),
  });
  return render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
}

describe("the not-found state", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("names the address and links home", async () => {
    const { container } = show("/projects/helsinki/nonexistent-section");
    // The page is nothing but this state, so its title is the page's one heading (T-3150).
    expect(await screen.findByRole("heading", { level: 1, name: en.app.notFound.title })).toBeInTheDocument();
    expect(screen.getByText(/nonexistent-section/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.app.notFound.home })).toHaveAttribute("href", "/");
    await expectNoViolations(container);
  });

  it("shows a hostile address as text, not as markup", async () => {
    const { container } = show("/projects/helsinki/<img src=x onerror=alert(1)>");
    await screen.findByText(en.app.notFound.title);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("onerror");
  });

  it("has a word for every locale the organisation offers", async () => {
    for (const locale of SUPPORTED_LOCALES) {
      await i18n.changeLanguage(locale);
      const { container, unmount } = show("/nowhere");
      await screen.findByRole("link");
      expectNoRawKeys(container);
      unmount();
    }
  });
});
