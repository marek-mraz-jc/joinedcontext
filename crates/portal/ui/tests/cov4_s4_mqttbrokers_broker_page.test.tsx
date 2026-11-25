// T-7706: MqttBrokerPage: the 404 page waits while the caller's grants are read, and a device added on a
// broker with its own hostname shows the mosquitto_pub example built by the page.
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { MqttBrokerPage } from "../src/pages/mqttbrokers/MqttBrokerPage";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "espoo";
const API = "joinedcontext.com/v1alpha1";
const d = en.mqttbrokers.device;
const EC = {
  kty: "EC", crv: "P-256", kid: "k1",
  x: "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
  y: "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
};
const BROKER = {
  apiVersion: API, kind: "MqttBroker", metadata: { name: "cameras", namespace: PROJECT },
  spec: { hostname: "mqtt.espoo.example", roles: [{ name: "cams", serviceAccounts: [], publish: ["cameras/${clientid}/#"] }] },
  status: { phase: "Live" },
};
const OPTIONS = {
  mayAdd: true,
  roles: [{ name: "cams", publish: ["cameras/${clientid}/#"] }],
  tokenUrl: "https://id.example/realms/jc/protocol/openid-connect/token",
  issuer: "https://id.example/realms/jc",
};

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("MqttBrokerPage edges (T-7706)", () => {
  it("keeps loading, not the refusal, on a 404 while the caller's grants are still being read", async () => {
    let brokerAnswered = false;
    let grant: (response: Response) => void = () => undefined;
    renderPage(<MqttBrokerPage project={PROJECT} name="cameras" />, {
      path: `/projects/${PROJECT}/mqttbrokers/cameras`,
      answer: async (url) => {
        if (url.pathname.endsWith("/permissions/me")) return new Promise<Response>((resolve) => { grant = resolve; });
        if (url.pathname.endsWith("/mqttbrokers/cameras")) {
          brokerAnswered = true;
          return json({ title: "Not Found", status: 404, detail: "no such broker" }, 404);
        }
        return undefined;
      },
    });
    await waitFor(() => expect(brokerAnswered).toBe(true));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.getAllByText(en.app.loading).length).toBeGreaterThan(0);
    expect(screen.queryByText(en.mqttbrokers.page.refused.purpose)).toBeNull();
    grant(json({ project: PROJECT, grants: [] }));
    await waitFor(() => expect(screen.queryAllByText(en.app.loading)).toHaveLength(0));
  });

  it("builds the publish example from the broker's own address when a device is proposed", async () => {
    const sent: string[] = [];
    renderPage(<MqttBrokerPage project={PROJECT} name="cameras" />, {
      path: `/projects/${PROJECT}/mqttbrokers/cameras`,
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) {
          return json({ project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker", "ServiceAccount"], verbs: ["read", "propose"] } }] });
        }
        if (url.pathname.endsWith("/mqttbrokers/cameras/devices")) {
          if (request.method === "GET") return json(OPTIONS);
          sent.push(await request.text());
          return json({ metadata: { name: "pr-9" }, status: { phase: "PendingApproval" } }, 202);
        }
        if (url.pathname.endsWith("/mqttbrokers/cameras")) return json(BROKER);
        const plural = url.pathname.split("/").pop() ?? "";
        return ["datasources", "pipelines", "endpoints", "policies", "serviceaccounts"].includes(plural) ? json(list([])) : undefined;
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: d.open }));
    fireEvent.change(screen.getByLabelText(new RegExp(d.name)), { target: { value: "dev1" } });
    fireEvent.click(screen.getByRole("radio", { name: new RegExp(d.paste) }));
    fireEvent.change(screen.getByLabelText(new RegExp(d.publicKey)), { target: { value: JSON.stringify({ keys: [EC] }) } });
    fireEvent.click(screen.getByRole("button", { name: d.propose }));
    await screen.findByText(d.signInTitle);
    expect(sent).toHaveLength(1);
    const example = await screen.findAllByText(/mosquitto_pub -h mqtt\.espoo\.example -p 8883/);
    expect(example.length).toBeGreaterThan(0);
    expect(example[0].textContent).toContain("-i espoo-dev1");
    expect(example[0].textContent).toContain("cameras/espoo-dev1/data");
  });
});
