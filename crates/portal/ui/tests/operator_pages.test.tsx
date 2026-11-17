/**
 * T-7140 (T-7129 O4b): the installation operators' pages. Hosts show their state and what they wait
 * for, with the day's certificate slots; a removal is asked for as a Change. A Change shows its
 * proposer that another operator decides it, and a refusal shows the server's sentence. The audit
 * lists every row. Organizations list their ids with the TXT record, and adding a domain runs the
 * challenge, the mint and a HostAdd. The operators' sections are in the menu for an operator alone.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { JSX } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OperatorAudit } from "../src/pages/operator/OperatorAudit";
import { OperatorChanges } from "../src/pages/operator/OperatorChanges";
import { OperatorHosts } from "../src/pages/operator/OperatorHosts";
import { OperatorInstallation } from "../src/pages/operator/OperatorInstallation";
import { OperatorOrganizations } from "../src/pages/operator/OperatorOrganizations";
import { HOST_STATES, stateTone, when } from "../src/pages/operator/operatorApi";
import { AdminNav } from "../src/pages/organization/AdminNav";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ME = "b7c1e0f4";
const OPERATOR = { project: "org", bootstrap: true, grants: [] } as unknown as Effective;
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;
const REFUSED = {
  type: "https://joinedcontext.com/problems/forbidden",
  title: "Forbidden",
  status: 403,
  detail: "operator Changes are for a person of the installation's operators' group (T-7129 §4)",
};

const HOSTS = {
  items: [
    { host: "smartcity.bbsk.sk", organization: "bbsk", organizationId: "sk_bbsk", role: "working", state: "served", detail: null, detailAt: null, updatedAt: "2026-11-16T09:00:00Z" },
    {
      host: "opendata.bbsk.sk",
      organization: "bbsk",
      organizationId: "sk_bbsk",
      role: "opendata",
      state: "dns-ok",
      detail: "waiting for a certificate slot until 2026-11-18 09:00 UTC",
      detailAt: "2026-11-17T09:00:00Z",
      updatedAt: "2026-11-17T09:00:00Z",
    },
  ],
  certificates: { usedToday: 10, perDay: 10, perDomainWeek: 20, nextFreeAt: "2026-11-18T09:00:00Z" },
};

const CHANGES = {
  items: [
    { id: "0b9d2f4e-1a2b-4c3d-8e9f-001122334455", kind: "HostAdd", payload: { host: "api.bbsk.sk", organization: "bbsk", role: "api" }, state: "proposed", proposedBy: ME, proposedAt: "2026-11-17T08:00:00Z", decidedBy: null, decidedAt: null },
    { id: "1c0e3a5f-2b3c-4d4e-9fa0-112233445566", kind: "HostRemove", payload: { host: "old.bbsk.sk" }, state: "proposed", proposedBy: "5f1c2b9e-0d3a-4c7e-9a41-2b6f8e0c1d23", proposedByName: "Jana Nováková", proposedAt: "2026-11-17T08:30:00Z", decidedBy: null, decidedAt: null },
  ],
};

type Seen = { method: string; path: string; body?: string };

function render(element: JSX.Element, answers: (path: string, method: string, body: string) => Response | undefined, permissions = OPERATOR) {
  const seen: Seen[] = [];
  renderPage(element, {
    path: "/organization/operator-hosts",
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

describe("hosts", () => {
  it("shows each host's state, what it waits for and the certificate slots", async () => {
    render(<OperatorHosts />, (path) => (path === "/api/v1/operator/hosts" ? json(HOSTS) : undefined));
    expect(await screen.findAllByText("smartcity.bbsk.sk")).not.toHaveLength(0);
    expect(screen.getAllByText(en.operator.state.served).length).toBeGreaterThan(0);
    expect(screen.getAllByText(en.operator.state["dns-ok"]).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Waiting for a certificate slot until 2026-11-18 09:00 UTC").length).toBeGreaterThan(0);
    // The role and the Organization ride under the host, so a phone card keeps them.
    expect(screen.getAllByText(`${en.operator.role.working} · bbsk`).length).toBeGreaterThan(0);
    expect(screen.getByText("10 of 10 new certificates requested in the last 24 hours.")).toBeTruthy();
    await expectNoAxeViolations(document.body);
  });

  it("says what to do when the registry is empty", async () => {
    render(<OperatorHosts />, (path) =>
      path === "/api/v1/operator/hosts" ? json({ items: [], certificates: { ...HOSTS.certificates, usedToday: 0, nextFreeAt: null } }) : undefined,
    );
    expect(await screen.findByText(en.operator.hosts.empty)).toBeTruthy();
  });

  it("shows a refusal as the server's sentence", async () => {
    render(<OperatorHosts />, (path) => (path === "/api/v1/operator/hosts" ? json(REFUSED, 403) : undefined), ADMIN);
    expect(await screen.findByText(REFUSED.detail, { exact: false })).toBeTruthy();
  });

  it("asks for a removal as a HostRemove Change", async () => {
    const seen = render(<OperatorHosts />, (path, method) => {
      if (path === "/api/v1/operator/hosts") return json(HOSTS);
      if (path === "/api/v1/operator/changes" && method === "POST") return json({ ...CHANGES.items[1], kind: "HostRemove" }, 201);
      return undefined;
    });
    const remove = await screen.findAllByRole("button", { name: en.operator.hosts.remove });
    fireEvent.click(remove[0]);
    fireEvent.click(await screen.findByRole("button", { name: en.operator.hosts.removeConfirm }));
    await waitFor(() => expect(screen.getByText(en.operator.hosts.removalProposed)).toBeTruthy());
    const post = seen.find((s) => s.method === "POST" && s.path === "/api/v1/operator/changes");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ kind: "HostRemove", host: "smartcity.bbsk.sk" });
  });
});

describe("changes", () => {
  it("tells the proposer another operator decides, approves another's, and shows a refusal", async () => {
    const seen = render(<OperatorChanges />, (path, method) => {
      if (path === "/api/v1/operator/changes" && method === "GET") return json(CHANGES);
      if (path.endsWith("/approve")) return json({ title: "Forbidden", status: 403, detail: "the operator who asked for this Change is no operator any more; reject it and ask again" }, 403);
      return undefined;
    });
    expect((await screen.findAllByText(en.operator.changes.ownHint)).length).toBeGreaterThan(0);
    // The asker by name, never the raw id; the Organization rides in the host's cell.
    expect(screen.getAllByText("Jana Nováková").length).toBeGreaterThan(0);
    expect(screen.queryByText("5f1c2b9e-0d3a-4c7e-9a41-2b6f8e0c1d23")).toBeNull();
    expect(screen.getAllByText(`${en.operator.changes.organization}: bbsk`).length).toBeGreaterThan(0);
    const approve = screen.getAllByRole("button", { name: en.operator.changes.approve });
    expect(approve).toHaveLength(1);
    fireEvent.click(approve[0]);
    expect(await screen.findByText("the operator who asked for this Change is no operator any more; reject it and ask again")).toBeTruthy();
    expect(seen.some((s) => s.method === "POST" && s.path === `/api/v1/operator/changes/${CHANGES.items[1].id}/approve`)).toBe(true);
    await expectNoAxeViolations(document.body);
  });

  it("says what to do when nothing is asked for", async () => {
    render(<OperatorChanges />, (path) => (path === "/api/v1/operator/changes" ? json({ items: [] }) : undefined));
    expect(await screen.findByText(en.operator.changes.empty)).toBeTruthy();
  });
});

describe("audit", () => {
  it("lists who did what, the reconciler named as the Portal", async () => {
    render(<OperatorAudit />, (path) =>
      path === "/api/v1/operator/audit"
        ? json({
            items: [
              { id: 2, at: "2026-11-17T09:00:00Z", actor: "reconciler", changeId: null, action: "certificate.created", before: null, after: { host: "opendata.bbsk.sk" }, sourceIp: null },
              { id: 3, at: "2026-11-17T09:30:00Z", actor: "5f1c2b9e-0d3a-4c7e-9a41-2b6f8e0c1d23", actorName: "Jana Nováková", changeId: null, action: "rejected HostRemove", before: null, after: null, sourceIp: null },
              { id: 1, at: "2026-11-17T08:00:00Z", actor: ME, changeId: CHANGES.items[0].id, action: "proposed HostAdd", before: null, after: { kind: "HostAdd", payload: { host: "api.bbsk.sk" } }, sourceIp: "10.0.0.7" },
            ],
          })
        : undefined,
    );
    expect((await screen.findAllByText(en.operator.action.certificate.created)).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Asked for: Add a host").length).toBeGreaterThan(0);
    expect(screen.getAllByText(en.operator.audit.reconciler).length).toBeGreaterThan(0);
    expect(screen.getAllByText("api.bbsk.sk").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Jana Nováková").length).toBeGreaterThan(0);
    expect(screen.queryByText("5f1c2b9e-0d3a-4c7e-9a41-2b6f8e0c1d23")).toBeNull();
  });
});

describe("organizations", () => {
  const ORGS = {
    items: [
      { name: "bbsk", title: "Banskobystrický kraj", domain: "bbsk.sk", hosts: { served: 1, "dns-ok": 1 } },
      { name: "presov", title: "Prešovský kraj", domain: null, hosts: {} },
    ],
    ids: [
      { id: "sk_bbsk", domain: "bbsk.sk", recordName: "_joinedcontext.bbsk.sk", recordValue: "jc-verify=c0ffee", requestedAt: "2026-11-16T09:00:00Z", mintedAt: "2026-11-16T09:10:00Z", organizations: ["bbsk"] },
      { id: "sk_zilina", domain: "zilina.sk", recordName: "_joinedcontext.zilina.sk", recordValue: "jc-verify=beef", requestedAt: "2026-11-17T09:00:00Z", mintedAt: null, organizations: [] },
    ],
  };

  it("lists the Organizations and each id's TXT record, and adds a domain and its first host", async () => {
    const seen = render(<OperatorOrganizations />, (path, method) => {
      if (path === "/api/v1/operator/organizations") return json(ORGS);
      if (path === "/api/v1/organization-ids/challenge")
        return json({ domain: "presov.sk", organizationId: "sk_presov", name: "_joinedcontext.presov.sk", value: "jc-verify=f00d", record: '_joinedcontext.presov.sk TXT "jc-verify=f00d"' });
      if (path === "/api/v1/organization-ids" && method === "POST") return json({ organizationId: "sk_presov", domain: "presov.sk", mintedAt: "2026-11-17T10:00:00Z" }, 201);
      if (path === "/api/v1/operator/changes" && method === "POST") return json(CHANGES.items[0], 201);
      return undefined;
    });
    expect((await screen.findAllByText("Banskobystrický kraj")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("1 served, 1 on their way").length).toBeGreaterThan(0);
    expect(screen.getAllByText('_joinedcontext.zilina.sk TXT "jc-verify=beef"').length).toBeGreaterThan(0);
    expect(screen.getAllByRole("button", { name: en.operator.ids.check })).toHaveLength(1);
    // An Organization without a domain says where it is set.
    expect(screen.getAllByText(en.operator.organizations.noDomain).length).toBeGreaterThan(0);
    expect(screen.getAllByRole("link", { name: en.operator.organizations.setDomain })[0]).toHaveAttribute("href", "/organization/settings");

    fireEvent.change(screen.getByLabelText(en.operator.add.domain), { target: { value: "Presov.sk" } });
    fireEvent.click(screen.getByRole("button", { name: en.operator.add.ask }));
    expect(await screen.findByText('_joinedcontext.presov.sk TXT "jc-verify=f00d"')).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: en.operator.add.mint }));
    expect(await screen.findByText("The domain is proved. Its Organization id is sk_presov.")).toBeTruthy();

    fireEvent.change(screen.getByLabelText(en.operator.add.host), { target: { value: "smartcity.presov.sk" } });
    fireEvent.change(screen.getByLabelText(en.operator.add.organization), { target: { value: "presov" } });
    // presov has no domain yet: the id stays home and the step says how to use it.
    expect(screen.getByText("Prešovský kraj has no domain yet", { exact: false })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: en.operator.add.propose }));
    expect(await screen.findByText(en.operator.add.proposed, { exact: false })).toBeTruthy();

    const challenge = seen.find((s) => s.path === "/api/v1/organization-ids/challenge");
    expect(JSON.parse(challenge?.body ?? "{}")).toEqual({ domain: "presov.sk" });
    const change = seen.find((s) => s.method === "POST" && s.path === "/api/v1/operator/changes");
    expect(JSON.parse(change?.body ?? "{}")).toEqual({
      kind: "HostAdd",
      host: "smartcity.presov.sk",
      organization: "presov",
      role: "working",
    });
    await expectNoAxeViolations(document.body);
  });

  it("sends the minted id with a host of the Organization whose domain it proves", async () => {
    const seen = render(<OperatorOrganizations />, (path, method) => {
      if (path === "/api/v1/operator/organizations") return json(ORGS);
      if (path === "/api/v1/organization-ids/challenge")
        return json({ domain: "bbsk.sk", organizationId: "sk_bbsk", name: "_joinedcontext.bbsk.sk", value: "jc-verify=c0ffee", record: '_joinedcontext.bbsk.sk TXT "jc-verify=c0ffee"' });
      if (path === "/api/v1/organization-ids" && method === "POST") return json({ organizationId: "sk_bbsk", domain: "bbsk.sk", mintedAt: "2026-11-17T10:00:00Z" }, 201);
      if (path === "/api/v1/operator/changes" && method === "POST") return json(CHANGES.items[0], 201);
      return undefined;
    });
    fireEvent.change(await screen.findByLabelText(en.operator.add.domain), { target: { value: "bbsk.sk" } });
    fireEvent.click(screen.getByRole("button", { name: en.operator.add.ask }));
    fireEvent.click(await screen.findByRole("button", { name: en.operator.add.mint }));
    expect(await screen.findByText("The domain is proved. Its Organization id is sk_bbsk.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText(en.operator.add.host), { target: { value: "portal.bbsk.sk" } });
    fireEvent.change(screen.getByLabelText(en.operator.add.organization), { target: { value: "bbsk" } });
    fireEvent.click(screen.getByRole("button", { name: en.operator.add.propose }));
    expect(await screen.findByText(en.operator.add.proposed, { exact: false })).toBeTruthy();
    const change = seen.find((s) => s.method === "POST" && s.path === "/api/v1/operator/changes");
    expect(JSON.parse(change?.body ?? "{}")).toMatchObject({ organization: "bbsk", organizationId: "sk_bbsk" });
  });

  it("names the field a refused host breaks", async () => {
    render(<OperatorOrganizations />, (path, method) => {
      if (path === "/api/v1/operator/organizations") return json(ORGS);
      if (path === "/api/v1/operator/changes" && method === "POST")
        return json({ title: "Unprocessable Content", status: 422, detail: "host: 'xbbsk.sk' is not under the domain bbsk.sk of Organization id sk_bbsk", errors: ["host: 'xbbsk.sk' is not under the domain bbsk.sk of Organization id sk_bbsk"] }, 422);
      return undefined;
    });
    fireEvent.change(await screen.findByLabelText(en.operator.add.host), { target: { value: "xbbsk.sk" } });
    fireEvent.change(screen.getByLabelText(en.operator.add.organization), { target: { value: "bbsk" } });
    fireEvent.click(screen.getByRole("button", { name: en.operator.add.propose }));
    expect(await screen.findByText("'xbbsk.sk' is not under the domain bbsk.sk of Organization id sk_bbsk", { exact: false })).toBeTruthy();
    expect(screen.getByLabelText(en.operator.add.host)).toHaveAttribute("aria-invalid", "true");
    expect(screen.queryByRole("alert", { name: /xbbsk/ })).toBeNull();
  });
});

describe("installation", () => {
  const E = en.operator.installation.email;
  const SMTP = { host: "smtp.example.org", port: 587, tls: "starttls", from: "noreply@example.org", user: "mailer", passwordSet: true };

  it("says what to do without an e-mail server and asks for one as an SmtpSet Change", async () => {
    const seen = render(<OperatorInstallation />, (path, method) => {
      if (path === "/api/v1/operator/installation") return json({ smtp: null, smtpChange: null });
      if (path === "/api/v1/operator/changes" && method === "POST")
        return json({ id: "c1", kind: "SmtpSet", payload: { smtp: { ...SMTP, newPassword: true } }, state: "proposed", proposedBy: ME, proposedAt: "2026-11-17T08:00:00Z", decidedBy: null, decidedAt: null }, 201);
      return undefined;
    });
    expect(await screen.findByText(E.empty)).toBeTruthy();
    expect(screen.getByText(E.unset)).toBeTruthy();
    expect(screen.getByRole("button", { name: E.test })).toHaveAttribute("aria-disabled", "true");
    for (const field of ["host", "tls", "port", "from", "user", "password"] as const) {
      expect(screen.getByText(E.hint[field])).toBeTruthy();
    }
    fireEvent.change(screen.getByLabelText(E.host, { exact: false }), { target: { value: "SMTP.Example.org" } });
    fireEvent.change(screen.getByLabelText(E.tls), { target: { value: "ssl" } });
    expect(screen.getByLabelText(E.port, { exact: false })).toHaveValue(465);
    fireEvent.change(screen.getByLabelText(E.from, { exact: false }), { target: { value: "noreply@example.org" } });
    fireEvent.change(screen.getByLabelText(E.user), { target: { value: "mailer" } });
    fireEvent.change(screen.getByLabelText(E.password), { target: { value: "app-pass" } });
    fireEvent.click(screen.getByRole("button", { name: E.propose }));
    expect(await screen.findByText(E.proposed, { exact: false })).toBeTruthy();
    const posted = seen.find((r) => r.method === "POST" && r.path === "/api/v1/operator/changes");
    expect(JSON.parse(posted?.body ?? "{}")).toEqual({
      kind: "SmtpSet",
      smtp: { host: "smtp.example.org", port: 465, tls: "ssl", from: "noreply@example.org", user: "mailer", password: "app-pass" },
    });
    expect(screen.getByLabelText(E.password)).toHaveValue("");
  });

  it("keeps the stored password unless a new one is typed, and sends a test e-mail", async () => {
    const seen = render(<OperatorInstallation />, (path, method) => {
      if (path === "/api/v1/operator/installation") return json({ smtp: SMTP, smtpChange: null });
      if (path === "/api/v1/operator/changes" && method === "POST") return json({ id: "c2", kind: "SmtpSet", payload: { smtp: { ...SMTP, newPassword: false } }, state: "proposed", proposedBy: ME, proposedAt: "2026-11-17T08:00:00Z", decidedBy: null, decidedAt: null }, 201);
      if (path === "/api/v1/operator/installation/test-email") return json({ sent: false, reason: "the SMTP server did not take the e-mail: check the host, port, TLS, user and password" });
      return undefined;
    });
    expect(await screen.findByText(E.set)).toBeTruthy();
    expect(screen.getByText(E.hint.passwordKept)).toBeTruthy();
    expect(screen.getByLabelText(E.password)).toHaveAttribute("placeholder", E.passwordStored);
    expect(screen.getByLabelText(E.host, { exact: false })).toHaveValue("smtp.example.org");
    fireEvent.change(screen.getByLabelText(E.port, { exact: false }), { target: { value: "2525" } });
    fireEvent.click(screen.getByRole("button", { name: E.propose }));
    await screen.findByText(E.proposed, { exact: false });
    const posted = JSON.parse(seen.find((r) => r.method === "POST" && r.path === "/api/v1/operator/changes")?.body ?? "{}");
    expect(posted.smtp.port).toBe(2525);
    expect("password" in posted.smtp).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: E.test }));
    expect(await screen.findByText(E.testFailed)).toBeTruthy();
    expect(screen.getByText(/The SMTP server did not take the e-mail/)).toBeTruthy();
  });

  it("names the field a refused setting breaks, and holds the form while a Change waits", async () => {
    render(<OperatorInstallation />, (path, method) => {
      if (path === "/api/v1/operator/installation") return json({ smtp: null, smtpChange: null });
      if (path === "/api/v1/operator/changes" && method === "POST")
        return json({ title: "Unprocessable Content", status: 422, detail: "the Change cannot be taken: '10.0.0.1' is no public host name", errors: ["smtp.host: '10.0.0.1' is no public host name of an SMTP server; give one, e.g. smtp.example.org"] }, 422);
      return undefined;
    });
    fireEvent.change(await screen.findByLabelText(E.host, { exact: false }), { target: { value: "10.0.0.1" } });
    fireEvent.change(screen.getByLabelText(E.from, { exact: false }), { target: { value: "noreply@example.org" } });
    fireEvent.click(screen.getByRole("button", { name: E.propose }));
    expect(await screen.findByText(/is no public host name of an SMTP server/)).toBeTruthy();
    expect(screen.getByLabelText(E.host, { exact: false })).toHaveAttribute("aria-invalid", "true");
    // The summary names the field to fix and the focus lands on it.
    expect(screen.getByText(E.notAsked)).toBeTruthy();
    expect(screen.getByText(E.fixField.replace("{field}", E.host))).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText(E.host, { exact: false }));
    cleanup();
    const asked = { id: "c3", kind: "SmtpSet", payload: { smtp: { host: "smtp.next.example.org", port: 465, tls: "ssl", from: "noreply@example.org", user: "", newPassword: false } }, state: "proposed", proposedBy: "s1", proposedAt: "2026-11-17T10:00:00Z" };
    render(<OperatorInstallation />, (path) =>
      path === "/api/v1/operator/installation"
        ? json({ smtp: SMTP, smtpChange: "c3" })
        : path === "/api/v1/operator/changes"
          ? json({ items: [asked] })
          : undefined,
    );
    expect(await screen.findByText(E.waiting, { exact: false })).toBeTruthy();
    expect(screen.getByRole("button", { name: E.propose })).toHaveAttribute("aria-disabled", "true");
    // The waiting change and the server in force, side by side as text; no field to type in.
    expect(await screen.findByText("smtp.next.example.org")).toBeTruthy();
    expect(screen.getByText(E.askedFor)).toBeTruthy();
    expect(screen.getByText(E.inForce)).toBeTruthy();
    expect(within(screen.getByText(E.askedFor).parentElement as HTMLElement).getByText(E.hint.passwordKept)).toBeTruthy();
    expect(screen.queryByRole("textbox", { name: new RegExp(E.host) })).toBeNull();
  });

  it("shows an Organization administrator the server's refusal", async () => {
    render(<OperatorInstallation />, (path) => (path === "/api/v1/operator/installation" ? json(REFUSED, 403) : undefined), ADMIN);
    expect(await screen.findByText(/operators' group/)).toBeTruthy();
    expect(screen.queryByLabelText(E.host, { exact: false })).toBeNull();
  });
});

describe("the state badge and the time", () => {
  it("draws served as success, a waiting host as warning, and keeps an unreadable time as sent", () => {
    expect(HOST_STATES.map(stateTone)).toEqual(["info", "info", "info", "info", "success", "neutral"]);
    expect(when("not a time", "en")).toBe("not a time");
    expect(when("2026-11-17T09:00:00Z", "en")).toMatch(/2026/);
  });
});

describe("menu", () => {
  it("shows the operators' sections to an operator alone", async () => {
    render(<AdminNav project="org" onNavigate={() => undefined} />, () => undefined, {
      ...(ADMIN as object),
      bootstrap: true,
    } as unknown as Effective);
    const nav = await screen.findByRole("heading", { name: en.organization.group.operators });
    expect(nav).toBeTruthy();
    expect(within(document.body).getByRole("link", { name: en.organization.tab["operator-hosts"] })).toBeTruthy();
    expect(within(document.body).getByRole("link", { name: en.organization.tab["operator-installation"] })).toBeTruthy();
    cleanup();
    render(<AdminNav project="org" onNavigate={() => undefined} />, () => undefined, ADMIN);
    await screen.findByRole("heading", { name: en.organization.group.settings });
    expect(screen.queryByRole("heading", { name: en.organization.group.operators })).toBeNull();
  });
});
