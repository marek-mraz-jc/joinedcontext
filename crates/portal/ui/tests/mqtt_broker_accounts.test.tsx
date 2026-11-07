/**
 * T-5590 (MF-54): a role's devices are picked, never typed. The New MQTT broker form offers the
 * project's ServiceAccounts as the API lists them for this person, proposes the one picked, and
 * says what to do when the list is empty. The API refuses any other name (mqtt_allowance_tests.rs).
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, list, renderRoute } from "./pageHarness";

vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value }: { value: string }) => <textarea aria-label="YAML" value={value} readOnly />,
}));

const account = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name, namespace: "helsinki" },
  spec: {},
});

/** A verdict as the Portal's check answers it (verdict_findings_tests.rs). */
function verdict(findings: { level: string; path: string; message: string }[]) {
  return { valid: findings.length === 0, verdict: { ok: findings.length === 0, findings, checkedAt: "", inputDigest: "sha256:1" } };
}

async function openForm(accounts: string[], findings: { level: string; path: string; message: string }[] = []) {
  const checks: unknown[] = [];
  await renderRoute({
    path: "/projects/helsinki/mqttbrokers/new?lang=en",
    answer: (path, request) => {
      if (path.endsWith("/helsinki/serviceaccounts")) return jsonResponse(list(accounts.map(account)));
      if (request.method === "POST" && new URL(request.url).searchParams.get("dryRun") === "All") {
        void request.clone().json().then((body) => checks.push(body));
        return jsonResponse(verdict(findings));
      }
      if (request.method === "PUT" && path.includes("/drafts/MqttBroker/")) {
        return request
          .clone()
          .json()
          .then((body: { manifest: unknown }) =>
            jsonResponse({ project: "helsinki", kind: "MqttBroker", name: "cameras", manifest: body.manifest, verdict: null, touchedBy: "demo", touchedKind: "person", version: 1, updatedAt: "" }),
          ) as unknown as Response;
      }
      return undefined;
    },
  });
  await waitFor(() => expect(document.getElementById("root_roles_0_serviceAccounts_0")).not.toBeNull(), { timeout: 4000 });
  return checks;
}

/** The role's first account, asked for each time: the picker redraws once its list arrives. */
const accountField = () => document.getElementById("root_roles_0_serviceAccounts_0") as HTMLSelectElement;

async function fill(account: string) {
  await userEvent.type(document.getElementById("root_name") as HTMLElement, "cameras");
  await userEvent.type(document.getElementById("root_roles_0_name") as HTMLElement, "camera");
  await userEvent.selectOptions(accountField(), account);
  await userEvent.click(screen.getByRole("button", { name: "Check" }));
}

describe("the broker form's service accounts (T-5590)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("offers only the accounts the API lists for this person and checks the one picked", async () => {
    const checks = await openForm(["cam-west", "cam-east"]);
    await waitFor(() => expect(within(accountField()).getAllByRole("option").map((o) => o.textContent)).toEqual(["Choose…", "cam-west", "cam-east"]));
    await fill("cam-east");
    await waitFor(() => expect(checks.length).toBeGreaterThan(0), { timeout: 5000 });
    const spec = (checks.at(-1) as { spec: { roles: { name: string; serviceAccounts: string[] }[] } }).spec;
    expect(spec.roles).toEqual([expect.objectContaining({ name: "camera", serviceAccounts: ["cam-east"] })]);
  });

  it("puts the server's refusal of an account on that account's field", async () => {
    const sentence = "spec.roles[0].serviceAccounts[0] names ServiceAccount 'cam-west', which does not exist in project 'helsinki'";
    await openForm(["cam-west"], [{ level: "error", path: "spec.roles[0].serviceAccounts[0]", message: sentence }]);
    await waitFor(() => expect(within(accountField()).queryByRole("option", { name: "cam-west" })).not.toBeNull());
    await fill("cam-west");
    const described = () =>
      (accountField().getAttribute("aria-describedby") ?? "").split(/\s+/).map((id) => document.getElementById(id)?.textContent ?? "").join(" ");
    await waitFor(() => expect(described()).toContain(sentence), { timeout: 5000 });
  });

  it("says where an account comes from when the project has none this person reads", async () => {
    await openForm([]);
    const hint = await screen.findByText(/No service account of this project that you can read/);
    const field = accountField();
    expect(field.getAttribute("aria-describedby")).toContain(hint.id);
    expect(hint.textContent).toContain("Add a device from the broker's page");
  });
});
