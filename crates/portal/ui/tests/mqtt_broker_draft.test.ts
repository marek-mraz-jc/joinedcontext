/**
 * T-6103: the broker page's edit draft. The topic and network checks mirror jc-core
 * (`mqtt/topics.rs`, `kinds/mqtt_broker.rs`), the derived mode follows the owner's v3 rule, and a
 * grant travels as a RoleBinding on the account.
 */
import { describe, expect, it } from "vitest";
import type { Manifest } from "../src/api/manifest";
import { changeWords } from "../src/pages/mqttbrokers/BrokerEditor";
import { accountChanges, blockers, draftOf, holdersOf, modeOf, rolesHeldBy, sourceProblem, topicProblem, withRole } from "../src/pages/mqttbrokers/brokerDraft";
import { fromMqttBrokerManifest, toMqttBrokerManifest } from "../src/schemas/mqttbroker";

const account = (name: string, roles: unknown[] = []): Manifest =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ServiceAccount", metadata: { name, namespace: "espoo" }, spec: { purpose: "x", roles } }) as Manifest;

describe("the broker draft", () => {
  it("refuses each topic jc-core refuses, and takes the ones it takes", () => {
    expect(topicProblem("cameras/${clientid}/frames", "publish")).toBeUndefined();
    expect(topicProblem("cameras/+/frames", "publish", true)).toBeUndefined();
    expect(topicProblem("commands/${clientid}/#", "subscribe")).toBeUndefined();
    expect(topicProblem("/cameras", "publish", true)).toBe("emptyLevel");
    expect(topicProblem("a//b", "publish", true)).toBe("emptyLevel");
    expect(topicProblem("#/a", "publish", true)).toBe("hash");
    expect(topicProblem("a/b+", "publish", true)).toBe("plus");
    expect(topicProblem("+/#", "publish", true)).toBe("everything");
    expect(topicProblem("$SYS/x", "publish", true)).toBe("system");
    expect(topicProblem("a/${tenant}", "publish", true)).toBe("placeholder");
    expect(topicProblem("a/b/c/d/e/f/g/h/i", "publish", true)).toBe("levels");
    expect(topicProblem("x".repeat(129), "publish", true)).toBe("length");
    expect(topicProblem("cameras/+/frames", "publish")).toBe("publishOwn");
    expect(topicProblem("commands/+/${clientid}", "subscribe")).toBe("subscribeOwn");
  });

  it("takes an IPv4 or IPv6 network, at most 32, each once", () => {
    expect(sourceProblem("203.0.113.0/24", [])).toBeUndefined();
    expect(sourceProblem("2001:db8::/48", [])).toBeUndefined();
    expect(sourceProblem("example.org", [])).toBe("format");
    expect(sourceProblem("10.0.0.0", [])).toBe("format");
    expect(sourceProblem("10.0.0.0/8", ["10.0.0.0/8"])).toBe("twice");
    expect(sourceProblem("10.0.0.0/8", Array.from({ length: 32 }, (_, i) => `10.${i}.0.0/16`))).toBe("tooMany");
  });

  it("derives the mode: subscribe rules make a relay, kept messages a capture", () => {
    expect(modeOf([{ publish: ["a/${clientid}"] }], false)).toBe("ingest");
    expect(modeOf([{ publish: ["a/${clientid}"] }], true)).toBe("capture");
    expect(modeOf([{ subscribe: ["c/${clientid}/#"] }], true)).toBe("relay");
  });

  it("reads holders from the role's list and the bindings for this broker and project only", () => {
    const accounts = [
      account("a", [{ role: "mqtt/cameras/cams", scope: { project: "espoo" } }]),
      account("b", [{ role: "mqtt/*/cams", scope: { project: "espoo" } }]),
      account("c", [{ role: "mqtt/other/cams", scope: { project: "espoo" } }]),
      account("d", [{ role: "mqtt/cameras/cams", scope: { project: "vantaa" } }]),
    ];
    expect(holdersOf({ name: "cams", serviceAccounts: ["old"] }, accounts, "cameras", "espoo")).toEqual([
      { account: "old", through: "legacy" },
      { account: "a", through: "binding" },
      { account: "b", through: "wildcard" },
    ]);
  });

  it("writes a grant and a revoke on the account and keeps its other bindings", () => {
    const keep = { role: "reader", scope: { project: "espoo" } };
    const accounts = [account("a", [keep, { role: "mqtt/cameras/cams", scope: { project: "espoo" } }]), account("b", [keep])];
    const draft = { ...draftOf({ name: "cameras", roles: [] }), grant: ["b/cams"], revoke: ["a/cams"] };
    const changed = accountChanges(draft, accounts, "cameras", "espoo");
    expect(changed.map((m) => [m.metadata.name, (m.spec as { roles: unknown[] }).roles])).toEqual([
      ["a", [keep]],
      ["b", [keep, { role: "mqtt/cameras/cams", scope: { project: "espoo" } }]],
    ]);
  });

  it("names what blocks a proposal: a role with no name, no account, no topic or no reason", () => {
    const draft = draftOf({ name: "cameras", roles: [{ name: "", serviceAccounts: [], publish: [], sharedTopics: true, reason: " " }] });
    expect(blockers(draft, () => []).map((b) => b.key)).toEqual(["roleName", "noAccount", "noTopic", "noReason"]);
    const ok = draftOf({ name: "cameras", roles: [{ name: "cams", publish: ["a/${clientid}"] }] });
    expect(blockers(ok, () => [{ account: "a", through: "binding" }])).toEqual([]);
  });

  it("says each pending change in words, a limit with its old and new value", () => {
    const t = (key: string, values?: Record<string, unknown>) => `${key}${values ? JSON.stringify(values) : ""}`;
    const stored = draftOf({ name: "cameras", roles: [{ name: "cams", publish: ["a/${clientid}"] }] });
    const draft = { ...stored, form: { ...stored.form, limits: { messagesPerSecond: 20 }, allowedSources: ["10.0.0.0/8"] }, grant: ["cam-4/cams"] };
    expect(changeWords(stored, draft, t)).toEqual([
      'mqttbrokers.edit.change.grant{"account":"cam-4","role":"cams"}',
      'mqttbrokers.edit.change.limit{"limit":"mqttbrokers.field.messagesPerSecond","from":10,"value":20}',
      'mqttbrokers.edit.change.sourceAdded{"source":"10.0.0.0/8"}',
    ]);
    expect(changeWords(stored, stored, t)).toEqual([]);
  });

  it("keeps the settings the form has no field for, and a role's shared topics with its reason", () => {
    const stored = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "MqttBroker",
      metadata: { name: "cameras", namespace: "espoo" },
      spec: {
        address: "customHostname",
        hostname: "mqtt.espoo.example",
        deviceAuth: [{ kind: "token" }],
        roles: [{ name: "counters", serviceAccounts: ["c"], publish: ["vendor/+/count"], sharedTopics: true, reason: "the vendor's one topic" }],
      },
    };
    const back = toMqttBrokerManifest("espoo", fromMqttBrokerManifest(stored), stored) as { spec: Record<string, unknown> };
    expect(back.spec.address).toBe("customHostname");
    expect(back.spec.hostname).toBe("mqtt.espoo.example");
    expect(back.spec.deviceAuth).toEqual([{ kind: "token" }]);
    expect(back.spec.roles).toEqual(stored.spec.roles);
  });
});

describe("a role's holders across the page (F-R2UI3-003, F-R2UI3-006)", () => {
  const bound = account("cam-9", [{ role: "mqtt/cameras/cams", scope: { project: "espoo" } }]);

  it("counts an account bound by a RoleBinding as holding the role", () => {
    const roles = [{ name: "cams", serviceAccounts: [], publish: ["cameras/${clientid}/#"] }, { name: "ops", serviceAccounts: ["cam-9"] }];
    expect(rolesHeldBy("cam-9", roles, [bound], "cameras", "espoo").map((role) => role.name)).toEqual(["cams", "ops"]);
    expect(rolesHeldBy("cam-9", roles, [bound], "other", "espoo").map((role) => role.name)).toEqual(["ops"]);
  });

  it("takes a pending grant along when its role is renamed", () => {
    const draft = { ...draftOf({ name: "cameras", roles: [{ name: "a", serviceAccounts: [] }] }), grant: ["cam-1/a"], revoke: ["cam-2/a"] };
    const renamed = withRole(draft, 0, { name: "b", serviceAccounts: [] });
    expect(renamed.form.roles?.[0].name).toBe("b");
    expect(renamed.grant).toEqual(["cam-1/b"]);
    expect(renamed.revoke).toEqual(["cam-2/b"]);
  });
});
