// T-7589: the operator lists with ten or more rows show a search; a search reads every column's text.
import type { JSX } from "react";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OperatorAudit } from "../src/pages/operator/OperatorAudit";
import { OperatorChanges } from "../src/pages/operator/OperatorChanges";
import { OperatorHosts } from "../src/pages/operator/OperatorHosts";
import { OperatorOrganizations } from "../src/pages/operator/OperatorOrganizations";
import { json, renderPage } from "./page_contract";

const OPERATOR = { project: "org", bootstrap: true, grants: [] } as unknown as Effective;

function render(element: JSX.Element, answers: (path: string) => Response | undefined): void {
  renderPage(element, {
    path: "/organization/operator-hosts",
    answer: async (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(OPERATOR);
      return answers(url.pathname);
    },
  });
}

async function search(text: string, index = 0): Promise<void> {
  const boxes = await screen.findAllByLabelText(en.dataGrid.searchRows);
  fireEvent.change(boxes[index], { target: { value: text } });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => cleanup());

const ORGS = Array.from({ length: 11 }, (_, i) => ({
  name: `org${i}`,
  title: i === 3 ? "Banskobystrický kraj" : null,
  domain: i === 4 ? "unique-domain.sk" : null,
  hosts: i === 5 ? { served: 2, "dns-ok": 1 } : {},
  approvals: { organization: `org${i}`, singlePerson: i === 6, peopleWithARole: 3, automationLogins: [] },
}));
const IDS = Array.from({ length: 11 }, (_, i) => ({
  id: `sk_id${i}`,
  domain: `d${i}.sk`,
  recordName: `_joinedcontext.d${i}.sk`,
  recordValue: `jc-verify=value${i}`,
  requestedAt: "2026-11-16T09:00:00Z",
  mintedAt: i % 2 === 0 ? "2026-11-16T09:10:00Z" : null,
  organizations: i === 2 ? ["org3"] : [],
}));

describe("operator organizations search", () => {
  it("finds an Organization by its title, domain, host summary and approvals, and an id by its record", async () => {
    render(<OperatorOrganizations />, (path) => (path === "/api/v1/operator/organizations" ? json({ items: ORGS, ids: IDS }) : undefined));
    await search("Banskobystrický");
    await waitFor(() => expect(screen.getAllByText("org3").length).toBeGreaterThan(0));
    expect(screen.queryAllByText("org7", { selector: "span" })).toHaveLength(0);
    await search("unique-domain", 0);
    await waitFor(() => expect(screen.getAllByText("unique-domain.sk").length).toBeGreaterThan(0));
    expect(screen.queryAllByText("org3", { selector: "span" })).toHaveLength(0);
    await search(en.operator.organizations.approvalsOn, 0);
    await waitFor(() => expect(screen.getAllByText("org6").length).toBeGreaterThan(0));
    expect(screen.queryByText("unique-domain.sk")).toBeNull();
    await search("zzz-nothing", 0);
    await waitFor(() => expect(screen.getAllByText(en.dataGrid.noMatch).length).toBeGreaterThan(0));

    await search("jc-verify=value1", 1);
    await waitFor(() => expect(screen.getAllByText(/jc-verify=value1"/).length).toBeGreaterThan(0));
    await search(en.operator.ids.minted, 1);
    await waitFor(() => expect(screen.getAllByText("sk_id0").length).toBeGreaterThan(0));
    await search("d3.sk sk_id3", 1);
    await search("sk_id2", 1);
    await waitFor(() => expect(screen.getAllByText("sk_id2").length).toBeGreaterThan(0));
    await search("d5.sk", 1);
    await waitFor(() => expect(screen.getAllByText("sk_id5").length).toBeGreaterThan(0));
    await search(en.operator.ids.waiting, 1);
    await waitFor(() => expect(screen.getAllByText("sk_id1").length).toBeGreaterThan(0));
  });
});

describe("operator hosts search", () => {
  it("searches a host by its name, the Organization's title, the state and the detail", async () => {
    const items = Array.from({ length: 10 }, (_, i) => ({
      host: `h${i}.bbsk.sk`,
      organization: "bbsk",
      organizationId: "sk_bbsk",
      role: "working",
      state: i === 2 ? "dns-ok" : "served",
      detail: i === 2 ? "waiting for a certificate slot" : null,
      detailAt: null,
      updatedAt: "2026-11-16T09:00:00Z",
    }));
    render(<OperatorHosts />, (path) => {
      if (path === "/api/v1/operator/hosts") return json({ items, certificates: { usedToday: 1, perDay: 10, perDomainWeek: 20, nextFreeAt: null } });
      if (path === "/api/v1/operator/organizations") return json({ items: ORGS.slice(0, 1).map((o) => ({ ...o, name: "bbsk", title: "Banskobystrický kraj" })), ids: [] });
      return undefined;
    });
    await search("Banskobystrický");
    await waitFor(() => expect(screen.getAllByText("h9.bbsk.sk").length).toBeGreaterThan(0));
    await search("waiting for a certificate");
    await waitFor(() => expect(screen.queryByText("h9.bbsk.sk")).toBeNull());
    expect(screen.getAllByText("h2.bbsk.sk").length).toBeGreaterThan(0);
    await search(en.operator.state["dns-ok"]);
    await waitFor(() => expect(screen.getAllByText("h2.bbsk.sk").length).toBeGreaterThan(0));
    expect(screen.queryByText("h0.bbsk.sk")).toBeNull();
  });
});

describe("operator changes search", () => {
  it("searches a Change by its host and Organization, state and the person who asked", async () => {
    const items = Array.from({ length: 10 }, (_, i) => ({
      id: `chg-${i}`,
      kind: "HostAdd",
      payload: { host: `c${i}.bbsk.sk`, organization: "bbsk" },
      state: i === 1 ? "applied" : "proposed",
      proposedBy: `user-${i}`,
      proposedByName: i === 4 ? "Zuzana Novakova" : null,
      proposedAt: "2026-11-17T08:00:00Z",
      decidedBy: null,
      decidedAt: null,
    }));
    render(<OperatorChanges />, (path) => {
      if (path === "/api/v1/operator/changes") return json({ items });
      if (path === "/api/v1/operator/organizations") return json({ items: [{ ...ORGS[0], name: "bbsk", title: "Banskobystrický kraj" }], ids: [] });
      return undefined;
    });
    await search("Banskobystrický");
    await waitFor(() => expect(screen.getAllByText("c9.bbsk.sk").length).toBeGreaterThan(0));
    await search("Zuzana");
    await waitFor(() => expect(screen.queryByText("c9.bbsk.sk")).toBeNull());
    expect(screen.getAllByText("c4.bbsk.sk").length).toBeGreaterThan(0);
    await search(en.operator.changeState.applied);
    await waitFor(() => expect(screen.getAllByText("c1.bbsk.sk").length).toBeGreaterThan(0));
    expect(screen.queryByText("c4.bbsk.sk")).toBeNull();
    await search(en.operator.kind.HostAdd);
    await waitFor(() => expect(screen.getAllByText("c4.bbsk.sk").length).toBeGreaterThan(0));
  });
});

describe("operator audit search", () => {
  it("names the subject of each action and searches by source address", async () => {
    const base = { changeId: null, before: null, sourceIp: null, actor: "someone", actorName: null };
    const items = [
      ...Array.from({ length: 7 }, (_, i) => ({ ...base, id: i + 1, at: "2026-11-17T09:00:00Z", action: "host.served", after: { host: `a${i}.bbsk.sk` } })),
      { ...base, id: 20, at: "2026-11-17T09:00:00Z", action: "proposed HostAdd", after: { payload: { host: "nested.bbsk.sk" } }, sourceIp: "10.9.8.7" },
      { ...base, id: 21, at: "2026-11-17T09:00:00Z", action: "applied SubjectAdd", after: { payload: { subject: "nested-subject" } } },
      { ...base, id: 22, at: "2026-11-17T09:00:00Z", action: "custom.thing", after: null, before: { smtp: { other: 1 } } },
      { ...base, id: 23, at: "2026-11-17T09:00:00Z", action: "custom.thing", after: ["list"], before: { subject: "before-subject" } },
    ];
    render(<OperatorAudit />, (path) => (path === "/api/v1/operator/audit" ? json({ items }) : undefined));
    expect((await screen.findAllByText("nested.bbsk.sk")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("nested-subject").length).toBeGreaterThan(0);
    expect(screen.getAllByText("before-subject").length).toBeGreaterThan(0);
    expect(screen.getAllByText(en.operator.action.host.replace("{state}", en.operator.state.served)).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.arrangeColumns }));
    const panel = within(screen.getByRole("dialog", { name: en.entityGrid.arrangeColumns }));
    fireEvent.click(panel.getByRole("checkbox", { name: `${en.entityGrid.showColumn}: ${en.operator.audit.source}` }));
    expect((await screen.findAllByText("10.9.8.7")).length).toBeGreaterThan(0);
  });
});
