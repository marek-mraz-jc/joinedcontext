/**
 * T-7013: Organization settings → Addresses. Every address the deployment routes for each of the
 * person's Organizations, under its title with its id, each with what it is for; an empty list
 * says what the operator does next; a failed read offers a retry.
 */
import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { OrganizationAddresses } from "../src/pages/organization/OrganizationAddresses";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const BBSK = {
  organization: "bbsk",
  title: "Banskobystrický kraj",
  organizationId: "sk_bbsk",
  addresses: [
    { kind: "sign-in", url: "https://login.smartcity.sk/realms/smartcity" },
    { kind: "mcp", url: "https://mcp.bbsk.sk/api/mcp/" },
    { kind: "portal", url: "https://portal.bbsk.sk/" },
    { kind: "shared", url: "https://smartcity.sk/sk_bbsk/" },
  ],
};

function renderAddresses(answer: () => Response) {
  let reads = 0;
  const result = renderPage(<OrganizationAddresses />, {
    path: "/organization/settings",
    answer: (url) => {
      if (url.pathname !== "/api/v1/organization/addresses") return undefined;
      reads += 1;
      return answer();
    },
  });
  return { ...result, reads: () => reads };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => cleanup());

describe("Organization addresses", () => {
  it("lists each address under the Organization's title, portal first, with its hint", async () => {
    const { container } = renderAddresses(() =>
      json({
        items: [
          BBSK,
          {
            organization: "nitra",
            title: "Nitra",
            addresses: [{ kind: "portal", url: "https://portal.nitra.sk/" }],
          },
        ],
      }),
    );
    const bbsk = await screen.findByRole("region", {
      name: "Banskobystrický kraj",
    });
    expect(within(bbsk).getByText("Organization id: sk_bbsk")).toBeTruthy();
    const rows = within(bbsk).getAllByRole("listitem");
    expect(rows.map((row) => row.querySelector("span")?.textContent)).toEqual([
      "Portal",
      "Shared host path",
      "MCP servers",
      "Sign-in",
    ]);
    const shared = within(bbsk).getByRole("link", {
      name: "https://smartcity.sk/sk_bbsk/",
    });
    expect(shared.getAttribute("href")).toBe("https://smartcity.sk/sk_bbsk/");
    expect(
      within(bbsk).getByText(en.organization.addresses.hint.mcp),
    ).toBeTruthy();
    const nitra = screen.getByRole("region", { name: "Nitra" });
    expect(
      within(nitra).getByText(en.organization.addresses.noId),
    ).toBeTruthy();
    await expectNoAxeViolations(container);
  });

  it("says what the operator does when the deployment lists none", async () => {
    renderAddresses(() => json({ items: [] }));
    expect(
      await screen.findByText(en.organization.addresses.empty),
    ).toBeTruthy();
    expect(screen.getByText(en.organization.addresses.emptyHint)).toBeTruthy();
    expect(screen.queryByRole("listitem")).toBeNull();
  });

  it("offers a retry when the read fails, and the retry reads again", async () => {
    let fail = true;
    const { reads } = renderAddresses(() =>
      fail
        ? new Response(
            JSON.stringify({
              type: "about:blank",
              title: "Unavailable",
              status: 503,
            }),
            {
              status: 503,
              headers: { "content-type": "application/problem+json" },
            },
          )
        : json({ items: [BBSK] }),
    );
    const retry = await screen.findByRole("button", {
      name: /try again|retry/i,
    });
    fail = false;
    fireEvent.click(retry);
    await waitFor(() => expect(reads()).toBeGreaterThanOrEqual(2));
    expect(
      await screen.findByRole("region", { name: "Banskobystrický kraj" }),
    ).toBeTruthy();
  });
});
