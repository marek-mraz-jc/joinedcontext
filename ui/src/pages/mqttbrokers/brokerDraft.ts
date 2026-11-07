import type { Manifest } from "../../api/manifest";
import type { MqttBrokerForm, MqttRoleForm } from "../../schemas/mqttbroker";
import { MAX_ALLOWED_SOURCES, SOURCE_RANGE } from "../../schemas/mqttbroker";
import { DNS1123 } from "../../schemas/kinds";

/**
 * The broker page's edit draft (T-6103): the checks a person meets as they type, and the changes
 * the proposal bar lists. They mirror jc-core (`kinds/mqtt_broker.rs`, `mqtt/topics.rs`) so a
 * typo is caught at the chip; the API still decides every proposal.
 */

/** jc-core `topics::MAX_FILTER_BYTES` and `MAX_LEVELS`. */
const MAX_FILTER_BYTES = 128;
const MAX_LEVELS = 8;
const CLIENT_ID = "${clientid}";

/** Why a topic filter is refused, as an i18n key under `mqttbrokers.edit.topic`, or undefined. */
export function topicProblem(filter: string, kind: "publish" | "subscribe", sharedTopics = false): string | undefined {
  if (filter === "" || new TextEncoder().encode(filter).length > MAX_FILTER_BYTES) return "length";
  if (filter.startsWith("$") && !filter.startsWith(CLIENT_ID)) return "system";
  const levels = filter.split("/");
  if (levels.length > MAX_LEVELS) return "levels";
  if (levels.every((level) => level === "+" || level === "#")) return "everything";
  for (const [i, level] of levels.entries()) {
    if (level === "") return "emptyLevel";
    if (level.includes("#") && (level !== "#" || i + 1 !== levels.length)) return "hash";
    if (level.includes("+") && level !== "+") return "plus";
    if (level.includes("${") && level !== CLIENT_ID) return "placeholder";
  }
  if (kind === "subscribe" || !sharedTopics) {
    if (!underClientId(filter)) return kind === "subscribe" ? "subscribeOwn" : "publishOwn";
  }
  return undefined;
}

/** jc-core `topics::under_client_id`: a `${clientid}` level comes before any wildcard. */
function underClientId(filter: string): boolean {
  for (const level of filter.split("/")) {
    if (level === CLIENT_ID) return true;
    if (level === "+" || level === "#") return false;
  }
  return false;
}

/** Why a network is refused (`mqttbrokers.edit.source.*`), or undefined. */
export function sourceProblem(source: string, sources: string[]): string | undefined {
  if (!new RegExp(SOURCE_RANGE).test(source)) return "format";
  if (sources.includes(source)) return "twice";
  if (sources.length >= MAX_ALLOWED_SOURCES) return "tooMany";
  return undefined;
}

/** The manifest mode the draft stands for: subscribe rules make a relay, kept messages a capture. */
export function modeOf(roles: MqttRoleForm[], keep: boolean): "ingest" | "capture" | "relay" {
  if (roles.some((role) => (role.subscribe ?? []).length > 0)) return "relay";
  return keep ? "capture" : "ingest";
}

/** Which roles of `broker` an account's RoleBindings grant here (`mqtt/<broker>/<role>`, and `mqtt/<star>/<role>` for every broker). */
export function boundRoles(account: Manifest, broker: string, project: string): { role: string; wildcard: boolean }[] {
  const roles = ((account.spec ?? {}) as { roles?: { role?: string; scope?: { project?: string } }[] }).roles ?? [];
  return roles.flatMap((binding) => {
    const [prefix, target, role] = (binding.role ?? "").split("/");
    if (prefix !== "mqtt" || !role || binding.scope?.project !== project) return [];
    if (target !== broker && target !== "*") return [];
    return [{ role, wildcard: target === "*" }];
  });
}

/** One holder of a role: through a binding on the account, or the deprecated list on the role. */
export interface Holder {
  account: string;
  through: "binding" | "wildcard" | "legacy";
  /** Granted in the draft, not proposed yet. */
  pending?: boolean;
}

/** Who holds `role` now: the role's legacy accounts and every readable account bound to it. */
export function holdersOf(role: MqttRoleForm, accounts: Manifest[], broker: string, project: string): Holder[] {
  const holders: Holder[] = (role.serviceAccounts ?? []).map((account) => ({ account, through: "legacy" }));
  for (const account of accounts) {
    const name = account.metadata.name;
    for (const bound of boundRoles(account, broker, project)) {
      if (bound.role !== role.name || holders.some((h) => h.account === name && h.through !== "legacy")) continue;
      holders.push({ account: name, through: bound.wildcard ? "wildcard" : "binding" });
    }
  }
  return holders;
}

/** The roles of `roles` that `account` holds: on the role's own list or through a RoleBinding (T-5872). */
export function rolesHeldBy<R extends MqttRoleForm>(account: string, roles: R[], accounts: Manifest[], broker: string, project: string): R[] {
  return roles.filter((role) => holdersOf(role, accounts, broker, project).some((holder) => holder.account === account));
}

/** The draft: the broker form plus who the page binds to which role and the "keep" switch. */
export interface Draft {
  form: MqttBrokerForm;
  keep: boolean;
  /** Bindings the draft adds or drops: `account/role` keys. */
  grant: string[];
  revoke: string[];
}

/** The draft with role `index` replaced: a renamed role takes its pending grants and revokes along. */
export function withRole(draft: Draft, index: number, role: MqttRoleForm): Draft {
  const from = draft.form.roles?.[index]?.name;
  const rekey = (keys: string[]) =>
    from === role.name
      ? keys
      : keys.map((key) => {
          const at = key.indexOf("/");
          return key.slice(at + 1) === from ? `${key.slice(0, at)}/${role.name ?? ""}` : key;
        });
  return {
    ...draft,
    form: { ...draft.form, roles: (draft.form.roles ?? []).map((r, i) => (i === index ? role : r)) },
    grant: rekey(draft.grant),
    revoke: rekey(draft.revoke),
  };
}

export function draftOf(form: MqttBrokerForm): Draft {
  return { form, keep: form.mode === "capture", grant: [], revoke: [] };
}

/** The draft's form with the derived mode, as `toMqttBrokerManifest` takes it. */
export function formOf(draft: Draft): MqttBrokerForm {
  return { ...draft.form, mode: modeOf(draft.form.roles ?? [], draft.keep) };
}

/** Each account manifest the draft changes, with its bindings added or dropped. */
export function accountChanges(draft: Draft, accounts: Manifest[], broker: string, project: string): Manifest[] {
  return accounts.flatMap((account) => {
    const name = account.metadata.name;
    const grants = draft.grant.filter((key) => key.startsWith(`${name}/`)).map((key) => key.slice(name.length + 1));
    const revokes = draft.revoke.filter((key) => key.startsWith(`${name}/`)).map((key) => key.slice(name.length + 1));
    if (grants.length === 0 && revokes.length === 0) return [];
    const spec = (account.spec ?? {}) as { roles?: { role?: string; scope?: { project?: string } }[] };
    const kept = (spec.roles ?? []).filter(
      (binding) => !(binding.scope?.project === project && revokes.some((role) => binding.role === `mqtt/${broker}/${role}`)),
    );
    const added = grants.map((role) => ({ role: `mqtt/${broker}/${role}`, scope: { project } }));
    return [{ ...account, status: undefined, spec: { ...spec, roles: [...kept, ...added] } } as Manifest];
  });
}

/** What blocks a proposal, as i18n keys with their values; empty when the draft may go. */
export function blockers(draft: Draft, holders: (role: MqttRoleForm, index: number) => Holder[]): { key: string; role: string }[] {
  const roles = draft.form.roles ?? [];
  const found: { key: string; role: string }[] = [];
  if (roles.length === 0) found.push({ key: "noRole", role: "" });
  roles.forEach((role, index) => {
    const label = role.name || String(index + 1);
    if (!role.name || !new RegExp(DNS1123).test(role.name)) found.push({ key: "roleName", role: label });
    else if (roles.some((other, j) => j !== index && other.name === role.name)) found.push({ key: "roleTwice", role: label });
    if (holders(role, index).length === 0) found.push({ key: "noAccount", role: label });
    if ((role.publish ?? []).length === 0 && (role.subscribe ?? []).length === 0) found.push({ key: "noTopic", role: label });
    const shared = (role as { sharedTopics?: boolean }).sharedTopics === true;
    if (shared && !((role as { reason?: string }).reason ?? "").trim()) found.push({ key: "noReason", role: label });
    const bad = [
      ...(role.publish ?? []).filter((filter) => topicProblem(filter ?? "", "publish", shared)),
      ...(role.subscribe ?? []).filter((filter) => topicProblem(filter ?? "", "subscribe")),
    ];
    if (bad.length > 0) found.push({ key: "badTopic", role: label });
  });
  return found;
}
