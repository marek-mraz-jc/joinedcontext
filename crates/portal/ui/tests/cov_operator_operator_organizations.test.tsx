// T-7589: coverage for OperatorOrganizations (drift rendering, search text extractors, IdList check button, role change in AddDomain)
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OperatorOrganizations } from "../src/pages/operator/OperatorOrganizations";
import { json, renderPage } from "./page_contract";

const OPERATOR = { project: "org", bootstrap: true, grants: [] } as unknown as Effective;

type Seen = { method: string; path: string; body?: string };

function renderOrgs(
  answers: (path: string, method: string, body: string) => Response | undefined,
  permissions = OPERATOR,
) {
  const seen: Seen[] = [];
  renderPage(<OperatorOrganizations />, {
    path: "/organization/operator-organizations",
    answer: async (url, request) => {
      const body = request.method === "GET" ? "" : await request.clone().text();
      seen.push({ method: request.method, path: url.pathname, body });
      if (url.pathname.endsWith("/permissions/me")) return json(permissions);
      return answers(url.pathname, request.method, body);
    },
  });
  return seen;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => cleanup());

describe("OperatorOrganizations coverage", () => {
  it("renders drift alert with missingFromForge and missingFromRegistry items", async () => {
    const data = {
      items: [
        {
          name: "bbsk",
          title: "Banskobystrický kraj",
          domain: "bbsk.sk",
          hosts: { served: 1 },
          approvals: { organization: "bbsk", singlePerson: false, peopleWithARole: 2, automationLogins: [] },
        },
      ],
      ids: [],
      drift: {
        missingFromForge: ["forge-org-1", "forge-org-2"],
        missingFromRegistry: ["reg-org-1"],
      },
    };

    renderOrgs((path) => {
      if (path === "/api/v1/operator/organizations") return json(data);
      return undefined;
    });

    expect(await screen.findByText(en.operator.organizations.driftTitle)).toBeInTheDocument();
    expect(screen.getByText("forge-org-1")).toBeInTheDocument();
    expect(screen.getByText("forge-org-2")).toBeInTheDocument();
    expect(screen.getByText("reg-org-1")).toBeInTheDocument();
  });

  it("shows error alert when mint check fails in IdList", async () => {
    const ids = [
      {
        id: "sk_fail",
        domain: "fail.sk",
        recordName: "_jc.fail.sk",
        recordValue: "jc-verify=fail",
        requestedAt: "2026-11-16T09:00:00Z",
        mintedAt: null,
        organizations: [],
      },
    ];

    renderOrgs((path, method) => {
      if (path === "/api/v1/operator/organizations") {
        return json({ items: [], ids });
      }
      if (path === "/api/v1/organization-ids" && method === "POST") {
        return json({ title: "Bad Request", status: 400, detail: "TXT record not found" }, 400);
      }
      return undefined;
    });

    const checkBtn = await screen.findByRole("button", { name: en.operator.ids.check });
    fireEvent.click(checkBtn);

    expect(await screen.findByText("TXT record not found")).toBeInTheDocument();
    expect(screen.getByText(en.operator.ids.notMinted)).toBeInTheDocument();
  });

  it("allows selecting role in AddDomain form and submitting with selected role", async () => {
    const orgs = [
      {
        name: "bbsk",
        title: "Banskobystrický kraj",
        domain: "bbsk.sk",
        hosts: {},
        approvals: { organization: "bbsk", singlePerson: false, peopleWithARole: 2, automationLogins: [] },
      },
    ];

    const seen = renderOrgs((path, method) => {
      if (path === "/api/v1/operator/organizations") return json({ items: orgs, ids: [] });
      if (path === "/api/v1/operator/changes" && method === "POST") {
        return json({ id: "chg-role", kind: "HostAdd", payload: {}, state: "proposed", proposedBy: "me", proposedAt: "2026-11-17T08:00:00Z" }, 201);
      }
      return undefined;
    });

    fireEvent.change(await screen.findByLabelText(en.operator.add.host), { target: { value: "data.bbsk.sk" } });
    fireEvent.change(screen.getByLabelText(en.operator.add.organization), { target: { value: "bbsk" } });

    const roleSelect = screen.getByLabelText(en.operator.add.role) as HTMLSelectElement;
    fireEvent.change(roleSelect, { target: { value: "opendata" } });
    expect(roleSelect.value).toBe("opendata");

    fireEvent.click(screen.getByRole("button", { name: en.operator.add.propose }));

    await waitFor(() => {
      const post = seen.find((s) => s.path === "/api/v1/operator/changes" && s.method === "POST");
      expect(post).toBeDefined();
      expect(JSON.parse(post?.body ?? "{}")).toMatchObject({ role: "opendata", host: "data.bbsk.sk", organization: "bbsk" });
    });
  });

  it("handles turn off approvals in SinglePersonCell, and shows error on failure", async () => {
    const orgs = [
      {
        name: "single-org",
        title: "Single Org",
        domain: "single.sk",
        hosts: {},
        approvals: { organization: "single-org", singlePerson: true, peopleWithARole: 1, automationLogins: [] },
      },
    ];

    const seen = renderOrgs((path, method) => {
      if (path === "/api/v1/operator/organizations") return json({ items: orgs, ids: [] });
      if (path === "/api/v1/operator/changes" && method === "POST") {
        return json({ title: "Bad Request", status: 400, detail: "Cannot turn off approvals" }, 400);
      }
      return undefined;
    });

    const turnOffBtn = (await screen.findAllByRole("button", { name: en.operator.organizations.turnOff }))[0];
    fireEvent.click(turnOffBtn);

    expect(await screen.findByText("Cannot turn off approvals")).toBeInTheDocument();
    const post = seen.find((s) => s.path === "/api/v1/operator/changes" && s.method === "POST");
    expect(JSON.parse(post?.body ?? "{}")).toMatchObject({ kind: "SinglePersonSet", organization: "single-org", singlePerson: false });
  });

  it("displays no organization message when organizations list is empty in AddDomain", async () => {
    renderOrgs((path) => {
      if (path === "/api/v1/operator/organizations") return json({ items: [], ids: [] });
      return undefined;
    });

    expect(await screen.findByText(en.operator.add.noOrganization)).toBeInTheDocument();
  });
});
