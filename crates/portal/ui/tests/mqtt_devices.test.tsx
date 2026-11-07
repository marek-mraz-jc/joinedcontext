/**
 * T-5532: Add device on the MQTT brokers page (PF-47). The key pair is made in the browser or its
 * public half pasted; a private key is refused before anything is sent; the private key is shown
 * once and never in a request; the role and topic dropdowns hold what the server offered.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { AddDevice } from "../src/pages/mqttbrokers/AddDevice";
import { generateDeviceKey, parsePublicKey, tokenCommand } from "../src/pages/mqttbrokers/devices";
import { json, renderPage } from "./page_contract";

const PROJECT = "espoo";
const X = "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU";
const Y = "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0";
const EC = { kty: "EC", crv: "P-256", x: X, y: Y, kid: "k1" };
const d = en.mqttbrokers.device;

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("a pasted public key", () => {
  it("reads a JWKS, one JWK and a PEM public key into public keys alone", async () => {
    expect(await parsePublicKey(JSON.stringify({ keys: [EC] }))).toEqual({
      ok: true,
      jwks: { keys: [{ kty: "EC", crv: "P-256", x: X, y: Y, kid: "k1", alg: "ES256", use: "sig" }] },
    });
    const single = await parsePublicKey(JSON.stringify({ kty: "EC", crv: "P-256", x: X, y: Y }));
    expect(single.ok && single.jwks.keys[0].kid).toMatch(/^[A-Za-z0-9_-]{43}$/);

    const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
    const spki = new Uint8Array(await crypto.subtle.exportKey("spki", pair.publicKey));
    const pem = `-----BEGIN PUBLIC KEY-----\n${btoa(String.fromCharCode(...spki))}\n-----END PUBLIC KEY-----`;
    const read = await parsePublicKey(pem);
    expect(read.ok && Object.keys(read.jwks.keys[0]).sort()).toEqual(["alg", "crv", "kid", "kty", "use", "x", "y"]);
  });

  it("refuses a private key, a symmetric key and what is no key, each with its reason", async () => {
    const cases: [string, string][] = [
      [JSON.stringify({ keys: [{ ...EC, d: "jpsQnnGQmL-YBIffH1136cspYG6-0iY7X1fCE9-E9LI" }] }), "private"],
      ["-----BEGIN PRIVATE KEY-----\nMIGH\n-----END PRIVATE KEY-----", "private"],
      [JSON.stringify({ keys: [{ kty: "oct", k: "c2VjcmV0", kid: "s" }] }), "private"],
      [JSON.stringify({ keys: [{ ...EC, crv: "P-384" }] }), "unsupported"],
      [JSON.stringify({ keys: [EC, EC] }), "kidTwice"],
      [JSON.stringify({ keys: Array.from({ length: 6 }, (_, i) => ({ ...EC, kid: `k${i}` })) }), "tooMany"],
      ["sensor password", "notKey"],
      ["   ", "empty"],
    ];
    for (const [text, problem] of cases) {
      expect(await parsePublicKey(text), text).toEqual({ ok: false, problem });
      expect(d.keyError[problem as keyof typeof d.keyError]).toBeTruthy();
    }
  });
});

describe("a key pair made in the browser", () => {
  it("is a P-256 public JWKS without private members and a PKCS#8 private key", async () => {
    const { jwks, privatePem } = await generateDeviceKey();
    expect(privatePem.startsWith("-----BEGIN PRIVATE KEY-----\n")).toBe(true);
    expect(jwks.keys).toHaveLength(1);
    expect(jwks.keys[0]).toMatchObject({ kty: "EC", crv: "P-256", alg: "ES256", use: "sig" });
    expect("d" in jwks.keys[0]).toBe(false);
    expect(await parsePublicKey(JSON.stringify(jwks))).toEqual({ ok: true, jwks });
  });

  it("signs in with the commands of the token endpoint, naming no secret", () => {
    const command = tokenCommand("https://id.example/realms/jc/protocol/openid-connect/token", "https://id.example/realms/jc", "espoo-dev1", { kty: "EC", kid: "k1", alg: "ES256", use: "sig" }, "dev1-key.pem");
    expect(command).toContain('"aud": "https://id.example/realms/jc"');
    expect(command).toContain("client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer");
    expect(command).toContain('open("dev1-key.pem")');
    expect(command).not.toMatch(/client_secret|PRIVATE KEY/);
  });
});

const OPTIONS = {
  mayAdd: true,
  roles: [
    { name: "cams", publish: ["cameras/${clientid}/#", "cameras/${clientid}/status"] },
    { name: "ops", publish: [] },
  ],
  tokenUrl: "https://id.example/realms/jc/protocol/openid-connect/token",
  issuer: "https://id.example/realms/jc",
};

function mount(answer: (body: unknown) => Response = () => json({ metadata: { name: "pr-9" }, status: { phase: "PendingApproval" } }, 202), options: unknown = OPTIONS) {
  const sent: string[] = [];
  renderPage(<AddDevice project={PROJECT} broker="cameras" example={(client, topic) => `mosquitto_pub -i ${client} -t '${topic}'`} />, {
    path: `/projects/${PROJECT}/mqttbrokers`,
    answer: async (url, request) => {
      if (!url.pathname.endsWith("/mqttbrokers/cameras/devices")) return undefined;
      if (request.method === "GET") return json(options);
      const body = await request.text();
      sent.push(body);
      return answer(JSON.parse(body));
    },
  });
  return sent;
}

async function openForm() {
  fireEvent.click(await screen.findByRole("button", { name: d.open }));
  fireEvent.change(screen.getByLabelText(new RegExp(d.name)), { target: { value: "dev1" } });
}

describe("Add device", () => {
  it("offers the private key once, sends only the public key and shows how the device signs in", async () => {
    const sent = mount();
    await openForm();
    const createObjectURL = vi.fn(() => "blob:key");
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL, revokeObjectURL: vi.fn() }));
    fireEvent.click(screen.getByRole("button", { name: d.generateNow }));
    const box = await screen.findByText(/-----BEGIN PRIVATE KEY-----/);
    const pem = box.textContent ?? "";
    expect(screen.getByText(d.onceTitle)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Download dev1-key.pem" }));
    expect(createObjectURL).toHaveBeenCalledTimes(1);

    expect(within(screen.getByLabelText(new RegExp(d.topic))).getAllByRole("option").map((o) => o.textContent)).toEqual(OPTIONS.roles[0].publish);
    fireEvent.click(screen.getByRole("button", { name: d.propose }));

    await screen.findByText(d.signInTitle);
    expect(sent).toHaveLength(1);
    const body = JSON.parse(sent[0]);
    expect(body).toMatchObject({ name: "dev1", role: "cams", topic: "cameras/${clientid}/#" });
    expect(sent[0]).not.toContain("PRIVATE KEY");
    expect(sent[0]).not.toContain(pem.split("\n")[1]);
    expect("d" in body.jwks.keys[0]).toBe(false);
    // Once proposed, the private key is gone from the page and from memory.
    expect(screen.queryByText(/-----BEGIN PRIVATE KEY-----/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Download dev1-key.pem" })).toBeNull();
    expect(screen.getByText(/client_assertion=/)).toBeInTheDocument();
    expect(screen.getByText(/mosquitto_pub -i espoo-dev1 -t 'cameras\/\$\{clientid\}\/data'/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: d.openChange })).toHaveAttribute("href", `/projects/${PROJECT}/approvals/pr-9`);
  });

  it("refuses a pasted private key in place and sends nothing", async () => {
    const sent = mount();
    await openForm();
    fireEvent.click(screen.getByRole("radio", { name: new RegExp(d.paste) }));
    fireEvent.change(screen.getByLabelText(new RegExp(d.publicKey)), { target: { value: JSON.stringify({ keys: [{ ...EC, d: "secret-part" }] }) } });
    fireEvent.click(screen.getByRole("button", { name: d.propose }));
    expect(await screen.findByText(d.keyError.private)).toBeInTheDocument();
    expect(sent).toHaveLength(0);
  });

  it("shows the server's refusal naming the field", async () => {
    mount(() => json({ title: "Unprocessable Content", status: 422, detail: "the topic is outside the role", errors: ["topic: role 'cams' publishes only to cameras/${clientid}/#"] }, 422));
    await openForm();
    fireEvent.click(screen.getByRole("radio", { name: new RegExp(d.paste) }));
    fireEvent.change(screen.getByLabelText(new RegExp(d.publicKey)), { target: { value: JSON.stringify({ keys: [EC] }) } });
    fireEvent.click(screen.getByRole("button", { name: d.propose }));
    const refusal = await screen.findByText(d.refused);
    await waitFor(() => expect(refusal.closest("[role=alert]")?.textContent).toContain("topic: role 'cams'"));
  });

  it("says who may add a device to a person the server offers nothing, and drops a bad name", async () => {
    mount(undefined, { mayAdd: false, roles: [] });
    expect(await screen.findByText(d.mayNot.replace("{broker}", "cameras"))).toBeInTheDocument();
    cleanup();
    const sent = mount();
    fireEvent.click(await screen.findByRole("button", { name: d.open }));
    fireEvent.change(screen.getByLabelText(new RegExp(d.name)), { target: { value: "Sensor_1" } });
    fireEvent.click(screen.getByRole("button", { name: d.propose }));
    expect(await screen.findByText(d.nameInvalid)).toBeInTheDocument();
    expect(sent).toHaveLength(0);
  });
});
