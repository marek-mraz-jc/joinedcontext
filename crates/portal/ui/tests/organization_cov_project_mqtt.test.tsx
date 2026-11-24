/**
 * Administration → Projects → MQTT: the action opens the project's form, and a project whose
 * allowance cannot be read shows a dash instead of a state.
 */
import { cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import type { Effective } from "../src/api/permissions";
import { ProjectMqtt, ProjectMqttState } from "../src/pages/organization/ProjectMqtt";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "admin", binding: "admins", rule: { kinds: ["RoleBinding", "Project"], verbs: ["read", "propose", "approve", "delete"] } }],
} as unknown as Effective;

const meter = (limit: number, used: number) => ({ limit, used, left: Math.max(limit - used, 0) });
const ALLOWANCE = {
  enabled: true,
  origin: "project",
  modes: ["ingest"],
  addresses: ["platform"],
  ports: [],
  brokers: meter(2, 1),
  connections: meter(10, 1),
  messagesPerSecond: meter(10, 1),
  maxMessageBytes: { limit: 65536 },
  portsInUse: [],
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

describe("the MQTT action of a project in Administration", () => {
  it("opens the project's form and closes it again on Escape", async () => {
    renderPage(<ProjectMqtt project="helsinki" title="Helsinki" />, {
      path: "/organization/projects",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
        if (url.pathname.endsWith("/mqtt/allowance")) return json(ALLOWANCE);
        return undefined;
      },
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "MQTT allowance of Helsinki" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows a dash for a project whose allowance is refused", async () => {
    renderPage(<ProjectMqttState project="helsinki" />, {
      path: "/organization/projects",
      answer: (url) => (url.pathname.endsWith("/mqtt/allowance") ? problem(403, "No access.") : undefined),
    });
    expect(await screen.findByText("—")).toBeInTheDocument();
  });
});
