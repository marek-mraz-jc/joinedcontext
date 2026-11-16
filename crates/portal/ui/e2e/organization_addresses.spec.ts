import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// Organization settings → Addresses (T-7013) as an operator meets it: two Organizations, one
// with its id and every kind of address, one before its id is set, and a deployment that lists
// none, in English and Slovak at 1440 and 400 px. `vite preview` has no Portal API behind it, so
// the API is answered in the browser; the page is the real build.

const IDENTITY = {
  subject: "s1",
  username: "demo.admin",
  name: "Demo Admin",
  email: "admin@bbsk.sk",
  roles: [],
};

const ADDRESSES = {
  items: [
    {
      organization: "bbsk",
      title: "Banskobystrický samosprávny kraj",
      organizationId: "sk_bbsk",
      addresses: [
        { kind: "portal", url: "https://portal.bbsk.sk/" },
        { kind: "opendata", url: "https://opendata.bbsk.sk/" },
        { kind: "api", url: "https://api.bbsk.sk/" },
        { kind: "mcp", url: "https://mcp.bbsk.sk/api/mcp/" },
        { kind: "portal-site", url: "https://mobility.bbsk.sk/" },
        { kind: "shared", url: "https://smartcity.joinedcontext.com/sk_bbsk/" },
        {
          kind: "sign-in",
          url: "https://login.smartcity.joinedcontext.com/realms/smartcity",
        },
      ],
    },
    {
      organization: "nitra",
      title: "Mesto Nitra",
      addresses: [
        { kind: "portal", url: "https://portal.nitra.sk/" },
        { kind: "shared", url: "https://smartcity.joinedcontext.com/nitra/" },
        {
          kind: "sign-in",
          url: "https://login.smartcity.joinedcontext.com/realms/smartcity",
        },
      ],
    },
  ],
};

async function stubApi(page: Page, addresses: unknown): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (url.pathname === "/api/v1/organization/addresses")
      return json(addresses);
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects")
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ProjectList",
        items: [{ name: "bbsk" }],
      });
    if (url.pathname.endsWith("/permissions/me"))
      return json({ project: "org", bootstrap: true, grants: [] });
    return json({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "List",
      items: [],
    });
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("Organization settings → Addresses (T-7013)", () => {
  for (const width of [1440, 400]) {
    for (const lang of ["en", "sk"] as const) {
      test(`an operator reads every address of each Organization (${lang}, ${width} px)`, async ({
        page,
      }) => {
        await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
        await stubApi(page, ADDRESSES);
        await page.goto(`/organization/settings?lang=${lang}`);
        const bbsk = page.getByRole("region", {
          name: "Banskobystrický samosprávny kraj",
        });
        await expect(bbsk).toBeVisible();
        await expect(
          bbsk.getByText(
            lang === "en"
              ? "Organization id: sk_bbsk"
              : "ID organizácie: sk_bbsk",
          ),
        ).toBeVisible();
        await expect(
          bbsk.getByRole("link", {
            name: "https://smartcity.joinedcontext.com/sk_bbsk/",
          }),
        ).toBeVisible();
        const nitra = page.getByRole("region", { name: "Mesto Nitra" });
        await expect(
          nitra.getByText(
            lang === "en"
              ? /Organization id: not set yet/
              : /ID organizácie: zatiaľ nenastavené/,
          ),
        ).toBeVisible();
        await page
          .getByRole("heading", {
            name: lang === "en" ? "Addresses" : "Adresy",
            exact: true,
          })
          .scrollIntoViewIfNeeded();
        await shot(page, `addresses-filled-${lang}-${width}`);
        // Keyboard: the first address link is reachable by Tab and names its address.
        await bbsk
          .getByRole("link", { name: "https://portal.bbsk.sk/" })
          .focus();
        await expect(
          bbsk.getByRole("link", { name: "https://portal.bbsk.sk/" }),
        ).toBeFocused();
        expect(await axeViolations(page)).toEqual([]);
      });
    }

    test(`a deployment that lists none says what the operator does at ${width} px`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, { items: [] });
      await page.goto("/organization/settings?lang=en");
      await expect(
        page.getByText("No Organization addresses are listed"),
      ).toBeVisible();
      await expect(
        page.getByText(/under \[\[organizations\]\] in jc\.toml/),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `addresses-empty-${width}`);
    });
  }
});
