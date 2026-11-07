// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/organization/NameById.tsx, src/api/personNames.ts.
/**
 * ADR-N-061 (T-5711): an administrator checks which manifests still name people by e-mail, then
 * proposes one Change per repository; the names behind ids are shown only to whoever updates
 * people or approves groups.
 */
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { NameById } from "../src/pages/organization/NameById";
import {
  isPersonId,
  personLabel,
  usePersonNames,
} from "../src/api/personNames";
import { expectNoViolations } from "./checks";

const JANA = "00000000-0000-4000-8000-00000000000a";
const PLAN = {
  repositories: [
    {
      repository: "organization",
      manifests: ["Group org/leads", "Policy helsinki/upload"],
    },
  ],
  unresolved: ["nobody@hel.fi"],
};

function stub(verbs: string[]): { url: string; method: string }[] {
  const sent: { url: string; method: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });
      if (url.pathname.endsWith("/permissions/me")) {
        return json({
          project: "org",
          bootstrap: false,
          grants: [{ rule: { kinds: ["Person"], verbs } }],
        });
      }
      sent.push({
        url: `${url.pathname}${url.search}`,
        method: request.method,
      });
      if (url.pathname === "/api/v1/organization/people/name-by-id") {
        const dryRun = url.searchParams.get("dryRun") === "true";
        return json(
          dryRun
            ? PLAN
            : {
                ...PLAN,
                repositories: PLAN.repositories.map((r) => ({
                  ...r,
                  change: "chg-00000042",
                })),
              },
        );
      }
      if (url.pathname === "/api/v1/organization/people/names") {
        return json({ items: [{ id: JANA, name: "Jana Kováčová" }] });
      }
      return json({ items: [] });
    }),
  );
  return sent;
}

function wrap(element: ReactElement) {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <I18nextProvider i18n={i18n}>
        <RouterProvider
          router={createRouter({
            routeTree: createRootRoute({ component: () => element }),
            history: createMemoryHistory({ initialEntries: ["/organization/people"] }),
          })}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

function Names({ ids }: { ids: string[] }) {
  const names = usePersonNames(ids);
  return (
    <ul>
      {ids.map((id) => (
        <li key={id}>{personLabel(id, names)}</li>
      ))}
    </ul>
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("naming people by id (ADR-N-061)", () => {
  it("checks first, lists the rewrites and the unknown addresses, then proposes one Change per repository", async () => {
    const sent = stub(["read", "update"]);
    const user = userEvent.setup();
    wrap(<NameById />);
    const section = await screen.findByRole("region", {
      name: en.organization.people.nameById.title,
    });
    const propose = within(section).getByRole("button", {
      name: en.organization.people.nameById.propose,
    });
    expect(propose).toHaveAttribute("aria-disabled", "true");
    await user.click(
      within(section).getByRole("button", {
        name: en.organization.people.nameById.check,
      }),
    );
    expect(
      await within(section).findByText(
        /Group org\/leads, Policy helsinki\/upload/,
      ),
    ).toBeInTheDocument();
    expect(within(section).getByText(/nobody@hel\.fi/)).toBeInTheDocument();
    await user.click(propose);
    expect(
      await within(section).findByRole("link", { name: "Change chg-00000042" }),
    ).toBeInTheDocument();
    expect(sent.filter((s) => s.method === "POST").map((s) => s.url)).toEqual([
      "/api/v1/organization/people/name-by-id?dryRun=true",
      "/api/v1/organization/people/name-by-id?dryRun=false",
    ]);
    await expectNoViolations(section);
  });

  it("is not offered to someone who does not update people", async () => {
    stub(["read"]);
    wrap(<NameById />);
    await waitFor(() =>
      expect(
        screen.queryByRole("region", {
          name: en.organization.people.nameById.title,
        }),
      ).toBeNull(),
    );
  });

  it("puts the name beside an id for a people administrator, and asks nobody else's names", async () => {
    stub(["read", "update"]);
    wrap(<Names ids={[JANA, "jana@hel.fi"]} />);
    expect(
      await screen.findByText(`Jana Kováčová (${JANA})`),
    ).toBeInTheDocument();
    expect(screen.getByText("jana@hel.fi")).toBeInTheDocument();
    cleanup();
    vi.unstubAllGlobals();
    const sent = stub(["read"]);
    wrap(<Names ids={[JANA]} />);
    expect(await screen.findAllByText(JANA)).not.toHaveLength(0);
    expect(
      sent.some((s) => s.url.startsWith("/api/v1/organization/people/names")),
    ).toBe(false);
  });

  it("knows an id from an address", () => {
    expect(isPersonId(JANA)).toBe(true);
    expect(isPersonId("jana@hel.fi")).toBe(false);
    expect(isPersonId(JANA.toUpperCase())).toBe(false);
  });
});
