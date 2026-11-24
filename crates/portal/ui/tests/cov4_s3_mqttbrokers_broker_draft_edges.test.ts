// T-7706: broker draft blockers: a broker with no role, a role named twice, a topic that no rule lets through.
import { describe, expect, it } from "vitest";
import { blockers, draftOf } from "../src/pages/mqttbrokers/brokerDraft";
import type { Holder } from "../src/pages/mqttbrokers/brokerDraft";
import type { MqttBrokerForm, MqttRoleForm } from "../src/schemas/mqttbroker";

const HOLDER: Holder = { account: "meters", through: "binding" };
const role = (name: string, publish: string[] = ["meters/${clientid}/up"], subscribe: string[] = []): MqttRoleForm => ({ name, publish, subscribe });
const form = (roles: MqttRoleForm[]): MqttBrokerForm => ({ name: "hall", roles });
const held = () => [HOLDER];

describe("what blocks a broker proposal", () => {
  it("asks for a role when there is none", () => {
    expect(blockers(draftOf(form([])), held)).toEqual([{ key: "noRole", role: "" }]);
  });

  it("names a role used twice and nothing for a clean draft", () => {
    expect(blockers(draftOf(form([role("meter"), role("meter")])), held)).toEqual([
      { key: "roleTwice", role: "meter" },
      { key: "roleTwice", role: "meter" },
    ]);
    expect(blockers(draftOf(form([role("meter"), role("gauge")])), held)).toEqual([]);
  });

  it("names a role whose filters are refused, on publish or on subscribe", () => {
    expect(blockers(draftOf(form([role("meter", ["meters/#"])])), held)).toEqual([{ key: "badTopic", role: "meter" }]);
    expect(blockers(draftOf(form([role("meter", ["meters/${clientid}/up"], ["a//b"])])), held)).toEqual([{ key: "badTopic", role: "meter" }]);
  });
});
