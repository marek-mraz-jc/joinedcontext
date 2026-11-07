import { useEffect, useId, useMemo, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { asManifests } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { usePermissions, useReadableList } from "../../api/permissions";
import { useProposal } from "../../api/proposal";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Badge, Button, Card, Checkbox, Field, fieldIds, InlineError, Input, Select, Textarea } from "../../components/ui";
import {
  CAPTURE_WINDOWS,
  DEDICATED_PORTS,
  durationWords,
  fromMqttBrokerManifest,
  toMqttBrokerManifest,
} from "../../schemas/mqttbroker";
import type { MqttRoleForm } from "../../schemas/mqttbroker";
import { accountChanges, blockers, draftOf, formOf, holdersOf, modeOf, sourceProblem, topicProblem, withRole } from "./brokerDraft";
import type { Draft, Holder } from "./brokerDraft";

type Measured = components["schemas"]["Measured"];

/** The broker page's sections, in page order; the nav links each one (T-6103). */
export const SECTIONS = ["live", "general", "access", "limits", "networks", "path", "connect"] as const;
export type SectionId = (typeof SECTIONS)[number];

/** jc-core's MqttLimits defaults (MF-54, T-5888), for a manifest that names none. */
export const LIMIT_DEFAULTS = {
  maxMessageBytes: 64 * 1024,
  messagesPerSecond: 10,
  maxConnections: 1000,
  connectionsPerAddress: 20,
};

/** jc-core's ceilings per limit (`MAX_MESSAGE_BYTES`, `MAX_MESSAGES_PER_SECOND`, `MAX_CONNECTIONS`). */
const LIMIT_MAX = { maxMessageBytes: 1024 * 1024, messagesPerSecond: 1000, maxConnections: 10_000, connectionsPerAddress: 10_000 };
type LimitKey = keyof typeof LIMIT_DEFAULTS;

export function Section({
  id,
  lead,
  dirty = false,
  children,
}: {
  id: SectionId;
  lead?: string;
  dirty?: boolean;
  children: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Card>
      <section id={`broker-${id}`} aria-labelledby={`broker-${id}-title`} className="flex scroll-mt-4 flex-col gap-3">
        <h2 id={`broker-${id}-title`} className="flex items-center gap-2 text-title font-semibold text-fg">
          {t(`mqttbrokers.page.section.${id}`)}
          {dirty ? <Badge tone="warning">{t("mqttbrokers.edit.unsaved")}</Badge> : null}
        </h2>
        {lead ? <p className="text-body text-fg-muted">{lead}</p> : null}
        {children}
      </section>
    </Card>
  );
}

/**
 * The broker's settings, edited where they are read (T-6103, owner mockup v3): General, Who may
 * connect, Limits and Networks hold the real controls, and one bar at the foot lists the pending
 * changes in words and proposes them as one Change. A grant goes on the account as a RoleBinding
 * `mqtt/<broker>/<role>` (T-5872), proposed with the broker; the server judges both again.
 */
export function BrokerEditor({
  project,
  manifest,
  measured,
  rolesNote,
  onDirty,
}: {
  project: string;
  manifest: Manifest;
  measured?: Measured;
  /** Shown under the roles: who-can-do-what for one account. */
  rolesNote?: ReactNode;
  onDirty?: (sections: SectionId[]) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const broker = manifest.metadata.name;
  const stored = useMemo(() => draftOf(fromMqttBrokerManifest(manifest)), [manifest]);
  const [draft, setDraft] = useState<Draft>(stored);
  const [change, setChange] = useState<Change | null>(null);
  const permissions = usePermissions(project);
  const editable = !permissions.checking && permissions.can("MqttBroker", "propose");
  const accountList = useReadableList(project, "serviceaccounts", "ServiceAccount");
  const accounts = useMemo(() => asManifests(accountList.data?.items ?? []), [accountList.data]);
  const proposal = useProposal(project, "mqttbrokers", (proposed) => {
    setChange(proposed);
    setDraft(stored);
  });

  const holders = (role: MqttRoleForm): Holder[] => {
    const now = holdersOf(role, accounts, broker, project).filter(
      (holder) => holder.through !== "binding" || !draft.revoke.includes(`${holder.account}/${role.name}`),
    );
    const granted = draft.grant
      .filter((key) => key.endsWith(`/${role.name}`))
      .map((key) => ({ account: key.slice(0, key.length - (role.name ?? "").length - 1), through: "binding" as const, pending: true }));
    return [...now, ...granted.filter((g) => !now.some((h) => h.account === g.account))];
  };
  const set = (form: Partial<Draft["form"]>) => setDraft((d) => ({ ...d, form: { ...d.form, ...form } }));
  const setRole = (index: number, role: MqttRoleForm) => setDraft((d) => withRole(d, index, role));

  const words = changeWords(stored, draft, t);
  const dirty: SectionId[] = [
    ...(stored.keep !== draft.keep || JSON.stringify(stored.form.capture) !== JSON.stringify(draft.form.capture) ? ["general" as const] : []),
    ...(JSON.stringify(stored.form.roles) !== JSON.stringify(draft.form.roles) || draft.grant.length + draft.revoke.length > 0
      ? ["access" as const]
      : []),
    ...(JSON.stringify(stored.form.limits ?? {}) !== JSON.stringify(draft.form.limits ?? {}) || stored.form.port !== draft.form.port
      ? ["limits" as const]
      : []),
    ...(JSON.stringify(stored.form.allowedSources ?? []) !== JSON.stringify(draft.form.allowedSources ?? []) ? ["networks" as const] : []),
  ];
  const dirtyKey = dirty.join(" ");
  useEffect(() => onDirty?.(dirtyKey === "" ? [] : (dirtyKey.split(" ") as SectionId[])), [dirtyKey, onDirty]);
  const blocked = blockers(draft, (role) => holders(role));
  const propose = () =>
    proposal.mutation.mutate({
      body: toMqttBrokerManifest(project, formOf(draft), manifest) as ResourceProposal,
      create: false,
      bundle: accountChanges(draft, accounts, broker, project) as unknown as ResourceProposal[],
    });

  return (
    <>
      {!editable && !permissions.checking ? (
        <Alert tone="info" title={t("mqttbrokers.edit.readOnlyTitle")} data-editor="read-only">
          {t("mqttbrokers.edit.readOnly")}
        </Alert>
      ) : null}
      {change ? <ChangeNotice change={change} project={project} /> : null}
      <Section id="general" dirty={dirty.includes("general")}>
        <General draft={draft} disabled={!editable} onChange={setDraft} />
      </Section>
      <Section id="access" lead={t("mqttbrokers.edit.accessLead")} dirty={dirty.includes("access")}>
        <div className="flex flex-col gap-3">
          {(draft.form.roles ?? []).map((role, index) => (
            <RoleCard
              key={index}
              index={index}
              role={role}
              mode={modeOf(draft.form.roles ?? [], draft.keep)}
              holders={holders(role)}
              accounts={accounts.map((a) => a.metadata.name)}
              disabled={!editable}
              onChange={(next) => setRole(index, next)}
              onRemove={() => set({ roles: (draft.form.roles ?? []).filter((_, i) => i !== index) })}
              onGrant={(account) =>
                setDraft((d) =>
                  d.revoke.includes(`${account}/${role.name}`)
                    ? { ...d, revoke: d.revoke.filter((k) => k !== `${account}/${role.name}`) }
                    : { ...d, grant: [...d.grant, `${account}/${role.name}`] },
                )
              }
              onDrop={(holder) =>
                setDraft((d) => {
                  const key = `${holder.account}/${role.name}`;
                  if (holder.through === "legacy") {
                    const roles = (d.form.roles ?? []).map((r, i) =>
                      i === index ? { ...r, serviceAccounts: (r.serviceAccounts ?? []).filter((a) => a !== holder.account) } : r,
                    );
                    return { ...d, form: { ...d.form, roles } };
                  }
                  return d.grant.includes(key)
                    ? { ...d, grant: d.grant.filter((k) => k !== key) }
                    : { ...d, revoke: [...d.revoke, key] };
                })
              }
            />
          ))}
          <Button
            className="self-start"
            disabled={!editable}
            disabledReason={editable ? undefined : t("mqttbrokers.edit.readOnly")}
            onClick={() => set({ roles: [...(draft.form.roles ?? []), { name: "", serviceAccounts: [], publish: [], subscribe: [] }] })}
          >
            {t("mqttbrokers.edit.addRole")}
          </Button>
          {rolesNote}
        </div>
      </Section>
      <Section id="limits" dirty={dirty.includes("limits")}>
        <Limits draft={draft} measured={measured} disabled={!editable} onChange={set} />
      </Section>
      <Section id="networks" dirty={dirty.includes("networks")}>
        <Networks sources={draft.form.allowedSources ?? []} disabled={!editable} onChange={(allowedSources) => set({ allowedSources })} />
      </Section>
      {words.length > 0 ? (
        <Card
          as="section"
          padding="md"
          aria-label={t("mqttbrokers.edit.bar")}
          data-editor="bar"
          className="sticky bottom-0 z-10 flex flex-col gap-2 pr-24 shadow-3 sm:pr-32 sm:flex-row sm:items-center sm:justify-between"
        >
          <div className="flex min-w-0 flex-col gap-1">
            <span className="font-semibold text-fg">{t("mqttbrokers.edit.pending", { count: words.length })}</span>
            <ul className="list-disc pl-4 text-caption text-fg-muted" data-editor="changes">
              {words.map((word) => (
                <li key={word}>{word}</li>
              ))}
            </ul>
            {proposal.error ? <InlineError>{proposal.error}</InlineError> : null}
          </div>
          <div className="flex shrink-0 flex-wrap gap-2">
            <Button
              onClick={() => {
                setDraft(stored);
                proposal.reset();
              }}
            >
              {t("mqttbrokers.edit.discard")}
            </Button>
            <Button
              variant="primary"
              onClick={propose}
              loading={proposal.mutation.isPending}
              disabled={blocked.length > 0}
              disabledReason={
                blocked.length > 0 ? t(`mqttbrokers.edit.block.${blocked[0]?.key}`, { role: blocked[0]?.role }) : undefined
              }
            >
              {t("mqttbrokers.edit.propose")}
            </Button>
          </div>
        </Card>
      ) : null}
    </>
  );
}

type Say = (key: string, values?: Record<string, unknown>) => string;

/** The pending changes in words, one line each. */
export function changeWords(stored: Draft, draft: Draft, t: Say): string[] {
  return [...roleWords(stored, draft, t), ...settingWords(stored, draft, t)];
}

/** Roles added, changed and removed, then the grants and revocations. */
function roleWords(stored: Draft, draft: Draft, t: Say): string[] {
  const words: string[] = [];
  const before = new Map((stored.form.roles ?? []).map((role) => [role.name, role]));
  const after = draft.form.roles ?? [];
  for (const role of after) {
    const old = role.name ? before.get(role.name) : undefined;
    if (!old) words.push(t("mqttbrokers.edit.change.roleAdded", { role: role.name || "…" }));
    else if (JSON.stringify(old) !== JSON.stringify(role)) words.push(t("mqttbrokers.edit.change.roleChanged", { role: role.name }));
  }
  for (const name of before.keys()) {
    if (!after.some((role) => role.name === name)) words.push(t("mqttbrokers.edit.change.roleRemoved", { role: name }));
  }
  for (const key of draft.grant) {
    const [account, role] = key.split("/");
    words.push(t("mqttbrokers.edit.change.grant", { account, role }));
  }
  for (const key of draft.revoke) {
    const [account, role] = key.split("/");
    words.push(t("mqttbrokers.edit.change.revoke", { account, role }));
  }
  return words;
}

/** Allowance mode, capture, limits, port and allowed sources. */
function settingWords(stored: Draft, draft: Draft, t: Say): string[] {
  const words: string[] = [];
  const after = draft.form.roles ?? [];
  const oldMode = modeOf(stored.form.roles ?? [], stored.keep);
  const newMode = modeOf(after, draft.keep);
  if (oldMode !== newMode) words.push(t("mqttbrokers.edit.change.mode", { mode: t(`mqttAllowance.mode.${newMode}`) }));
  else if (newMode === "capture" && JSON.stringify(stored.form.capture) !== JSON.stringify(draft.form.capture)) {
    words.push(t("mqttbrokers.edit.change.capture"));
  }
  for (const key of Object.keys(LIMIT_DEFAULTS) as LimitKey[]) {
    const was = stored.form.limits?.[key];
    const now = draft.form.limits?.[key];
    if (was !== now) {
      const from = was ?? LIMIT_DEFAULTS[key];
      words.push(t("mqttbrokers.edit.change.limit", { limit: t(`mqttbrokers.field.${key}`), from, value: now ?? LIMIT_DEFAULTS[key] }));
    }
  }
  if (stored.form.port !== draft.form.port) words.push(t("mqttbrokers.edit.change.port", { port: draft.form.port ?? t("mqttbrokers.edit.default") }));
  const was = stored.form.allowedSources ?? [];
  const now = draft.form.allowedSources ?? [];
  for (const source of now) if (!was.includes(source)) words.push(t("mqttbrokers.edit.change.sourceAdded", { source }));
  for (const source of was) if (!now.includes(source)) words.push(t("mqttbrokers.edit.change.sourceRemoved", { source }));
  return words;
}

function General({ draft, disabled, onChange }: { draft: Draft; disabled: boolean; onChange: (draft: Draft) => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const id = useId();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const relay = modeOf(draft.form.roles ?? [], false) === "relay";
  const capture = draft.form.capture ?? {};
  const setCapture = (next: typeof capture) => onChange({ ...draft, form: { ...draft.form, capture: { ...capture, ...next } } });
  return (
    <div className="flex flex-col gap-4">
      <p className="text-body" data-mode={modeOf(draft.form.roles ?? [], draft.keep)}>
        {t(`mqttAllowance.modeLong.${modeOf(draft.form.roles ?? [], draft.keep)}`)}
      </p>
      <Checkbox
        label={t("mqttbrokers.edit.keep")}
        checked={draft.keep && !relay}
        disabled={disabled || relay}
        disabledReason={relay ? t("mqttbrokers.edit.keepRelay") : disabled ? t("mqttbrokers.edit.readOnly") : undefined}
        onChange={(event) => onChange({ ...draft, keep: event.target.checked })}
      />
      <p className="-mt-3 text-caption text-fg-muted">{t("mqttbrokers.edit.hint.keepHint")}</p>
      {draft.keep && !relay ? (
        <div className="grid grid-cols-[minmax(0,1fr)] gap-3 sm:grid-cols-2">
          <Field id={`${id}-count`} label={t("mqttbrokers.field.messagesPerTopic")} help={t("mqttbrokers.edit.hint.countHint")}>
            <Input
              id={`${id}-count`}
              type="number"
              min={1}
              max={1000}
              disabled={disabled}
              title={disabled ? t("mqttbrokers.edit.readOnly") : undefined}
              value={capture.messagesPerTopic ?? 20}
              onChange={(event) => setCapture({ messagesPerTopic: wholeNumber(event.target.value) })}
            />
          </Field>
          <Field id={`${id}-window`} label={t("mqttbrokers.field.window")} help={t("mqttbrokers.edit.hint.windowHint")}>
            <Select id={`${id}-window`} disabled={disabled} title={disabled ? t("mqttbrokers.edit.readOnly") : undefined} value={capture.window ?? "PT1H"} onChange={(event) => setCapture({ window: event.target.value })}>
              {CAPTURE_WINDOWS.map((window) => (
                <option key={window} value={window}>
                  {durationWords(window, locale)}
                </option>
              ))}
            </Select>
          </Field>
        </div>
      ) : null}
      {draft.keep && !relay ? (
        <p className="text-body" data-kept="">
          {t("mqttbrokers.capture.kept", {
            count: capture.messagesPerTopic ?? 20,
            window: durationWords(capture.window ?? "PT1H", locale),
          })}
        </p>
      ) : null}
      <div data-qos="1">
        <span id={`${id}-qos`} className="font-semibold text-fg">
          {t("mqttbrokers.edit.qos")}
        </span>
        <ul aria-labelledby={`${id}-qos`} className="mt-1 flex flex-wrap gap-2">
          {(["0", "1", "2"] as const).map((level) => (
            <li key={level} data-level={level}>
              <Badge tone={level === "2" ? "neutral" : "success"}>
                {level === "2" ? <s>{t("mqttbrokers.edit.qosLevel", { level })}</s> : t("mqttbrokers.edit.qosLevel", { level })}
                {" · "}
                {t(level === "1" ? "mqttbrokers.edit.qosHighest" : level === "0" ? "mqttbrokers.edit.qosOffered" : "mqttbrokers.edit.qosNotOffered")}
              </Badge>
            </li>
          ))}
        </ul>
        <p className="mt-1 text-body text-fg-muted">{t("mqttbrokers.page.qosWords")}</p>
        <p className="text-caption text-fg-muted">{t("mqttbrokers.edit.qosFixed")}</p>
      </div>
      <div className="flex flex-col gap-1">
        <span className="font-semibold text-fg">{t("mqttbrokers.page.offline")}</span>
        <span className="text-body text-fg-muted">{t("mqttbrokers.page.offlineWords")}</span>
      </div>
    </div>
  );
}

function RoleCard({
  index,
  role,
  mode,
  holders,
  accounts,
  disabled,
  onChange,
  onRemove,
  onGrant,
  onDrop,
}: {
  index: number;
  role: MqttRoleForm;
  mode: string;
  holders: Holder[];
  accounts: string[];
  disabled: boolean;
  onChange: (role: MqttRoleForm) => void;
  onRemove: () => void;
  onGrant: (account: string) => void;
  onDrop: (holder: Holder) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const free = accounts.filter((account) => !holders.some((holder) => holder.account === account));
  const shared = role.sharedTopics === true;
  return (
    <Card padding="sm">
    <fieldset className="flex flex-col gap-3" data-role={role.name || index}>
      {/* The name field below shows the role's name; the legend names the group for a screen reader only. */}
      <legend className="sr-only">{role.name || t("mqttbrokers.edit.newRole")}</legend>
      <Field id={`${id}-name`} label={t("mqttbrokers.field.roleName")} help={t("mqttbrokers.edit.hint.roleNameHint")}>
        <Input id={`${id}-name`} className="max-w-md" value={role.name ?? ""} disabled={disabled} title={disabled ? t("mqttbrokers.edit.readOnly") : undefined} onChange={(event) => onChange({ ...role, name: event.target.value.trim() })} />
      </Field>
      <div className="flex flex-col gap-1">
        <span id={`${id}-accounts`} className="text-body font-medium text-fg">
          {t("mqttbrokers.acl.accounts")}
        </span>
        <ul aria-labelledby={`${id}-accounts`} className="flex flex-wrap gap-1" data-role-holders="">
          {holders.length === 0 ? <li className="text-caption text-fg-muted">{t("mqttbrokers.edit.noHolder")}</li> : null}
          {holders.map((holder) => (
            <li key={`${holder.account}-${holder.through}`}>
              <Chip
                label={holder.account}
                note={
                  holder.pending
                    ? t("mqttbrokers.edit.unsaved")
                    : holder.through === "binding"
                      ? undefined
                      : t(`mqttbrokers.edit.through.${holder.through}`)
                }
                removable={!disabled && holder.through !== "wildcard"}
                onRemove={() => onDrop(holder)}
              />
            </li>
          ))}
        </ul>
        {free.length > 0 && !disabled && role.name ? (
          <Select
            aria-label={t("mqttbrokers.edit.addAccount", { role: role.name })}
            value=""
            className="max-w-xs"
            onChange={(event) => event.target.value && onGrant(event.target.value)}
          >
            <option value="">{t("mqttbrokers.edit.addAccountOption")}</option>
            {free.map((account) => (
              <option key={account} value={account}>
                {account}
              </option>
            ))}
          </Select>
        ) : null}
        <span className="text-caption text-fg-muted">
          {accounts.length === 0 ? t("mqttbrokers.edit.noAccounts") : t("mqttbrokers.edit.hint.accountsHint")}
        </span>
      </div>
      <ChipInput
        label={t("mqttbrokers.acl.publish")}
        help={t("mqttbrokers.edit.hint.publishHint")}
        values={role.publish ?? []}
        disabled={disabled}
        problem={(value) => topicProblem(value, "publish", shared)}
        onChange={(publish) => onChange({ ...role, publish })}
      />
      <ChipInput
        label={t("mqttbrokers.acl.subscribe")}
        help={mode === "relay" ? t("mqttbrokers.edit.hint.subscribeHint") : t("mqttbrokers.edit.hint.subscribeMakesRelay")}
        values={role.subscribe ?? []}
        disabled={disabled}
        problem={(value) => topicProblem(value, "subscribe")}
        onChange={(subscribe) => onChange({ ...role, subscribe })}
      />
      <Checkbox
        label={t("mqttbrokers.edit.shared")}
        checked={shared}
        disabled={disabled}
        disabledReason={disabled ? t("mqttbrokers.edit.readOnly") : undefined}
        onChange={(event) => onChange({ ...role, sharedTopics: event.target.checked, reason: event.target.checked ? (role.reason ?? "") : undefined })}
      />
      <p className="-mt-2 text-caption text-fg-muted">{t("mqttbrokers.edit.sharedHint")}</p>
      {shared ? (
        <Field
          id={`${id}-reason`}
          label={t("mqttbrokers.edit.reason")}
          required
          help={t("mqttbrokers.edit.hint.reasonHint")}
          errors={(role.reason ?? "").trim() ? undefined : [t("mqttbrokers.edit.reasonMissing")]}
        >
          <Textarea id={`${id}-reason`} rows={2} disabled={disabled} title={disabled ? t("mqttbrokers.edit.readOnly") : undefined} value={role.reason ?? ""} onChange={(event) => onChange({ ...role, reason: event.target.value })} />
        </Field>
      ) : null}
      {!disabled ? (
        <Button variant="danger" size="sm" className="self-start" onClick={onRemove}>
          {t("mqttbrokers.edit.removeRole", { role: role.name || index + 1 })}
        </Button>
      ) : null}
    </fieldset>
    </Card>
  );
}

function Chip({ label, note, removable, onRemove }: { label: string; note?: string; removable: boolean; onRemove: () => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <span className="inline-flex max-w-full items-center gap-1 rounded-md border border-border bg-surface-subtle py-0.5 pl-2 text-caption">
      <span className="break-all font-mono">{label}</span>
      {note ? <span className="text-fg-muted">· {note}</span> : null}
      {removable ? (
        <Button variant="ghost" size="xs" aria-label={t("mqttbrokers.edit.remove", { value: label })} onClick={onRemove}>
          ×
        </Button>
      ) : (
        <span className="pr-2" />
      )}
    </span>
  );
}

/** A list of values as chips, each checked as it is typed; Enter or Add puts it on the list. */
export function ChipInput({
  label,
  help,
  values,
  disabled,
  problem,
  placeholder,
  onChange,
}: {
  label: string;
  help: string;
  values: string[];
  disabled: boolean;
  problem: (value: string) => string | undefined;
  placeholder?: string;
  onChange: (values: string[]) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [text, setText] = useState("");
  const typed = text.trim();
  const wrong = typed === "" ? undefined : values.includes(typed) ? "twice" : problem(typed);
  const add = () => {
    if (typed === "" || wrong) return;
    onChange([...values, typed]);
    setText("");
  };
  return (
    <Field id={`${id}-input`} label={label} help={help} errors={wrong ? [t(`mqttbrokers.edit.topic.${wrong}`)] : undefined}>
      <div className="flex max-w-2xl flex-col gap-2">
        {values.length > 0 ? (
          <ul aria-label={label} className="flex flex-wrap gap-1">
            {values.map((value) => (
              <li key={value}>
                <Chip
                  label={value}
                  note={problem(value) ? t(`mqttbrokers.edit.topic.${problem(value)}`) : undefined}
                  removable={!disabled}
                  onRemove={() => onChange(values.filter((v) => v !== value))}
                />
              </li>
            ))}
          </ul>
        ) : null}
        {!disabled ? (
          <div className="flex gap-2">
            <Input
              id={`${id}-input`}
              className="min-w-0 flex-1 font-mono"
              value={text}
              placeholder={placeholder ?? t("mqttbrokers.edit.hint.topicExample")}
              aria-invalid={wrong ? true : undefined}
              aria-describedby={described(`${id}-input`, wrong !== undefined)}
              onChange={(event) => setText(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  add();
                }
              }}
            />
            <Button onClick={add} disabled={typed === "" || wrong !== undefined} disabledReason={typed === "" ? t("mqttbrokers.edit.typeFirst") : undefined}>
              {t("mqttbrokers.edit.add")}
            </Button>
          </div>
        ) : null}
      </div>
    </Field>
  );
}

function Limits({
  draft,
  measured,
  disabled,
  onChange,
}: {
  draft: Draft;
  measured?: Measured;
  disabled: boolean;
  onChange: (form: Partial<Draft["form"]>) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const id = useId();
  const number = new Intl.NumberFormat(i18n.resolvedLanguage ?? i18n.language ?? "en", { maximumFractionDigits: 1 });
  const limits = { ...LIMIT_DEFAULTS, ...draft.form.limits };
  const use: Partial<Record<LimitKey, number>> = measured?.measured
    ? {
        maxConnections: measured.accounts.reduce((sum, account) => sum + account.connected, 0),
      }
    : {};
  const setLimit = (key: LimitKey, value: number | undefined) =>
    onChange({ limits: Object.fromEntries(Object.entries({ ...draft.form.limits, [key]: value }).filter(([, v]) => v !== undefined)) });
  const field = (key: LimitKey) => {
    const value = draft.form.limits?.[key];
    const wrong = value !== undefined && (value < 1 || value > LIMIT_MAX[key]);
    return (
      <Field
        key={key}
        id={`${id}-${key}`}
        label={t(`mqttbrokers.edit.limit.${key}`)}
        help={t(`mqttbrokers.page.limitWords.${key}`)}
        errors={wrong ? [t("mqttbrokers.edit.outside", { max: number.format(LIMIT_MAX[key]) })] : undefined}
      >
        <div className="flex flex-col gap-1" data-limit={key}>
          <div className="flex max-w-64 items-center gap-2">
            <Input
              id={`${id}-${key}`}
              aria-invalid={wrong || undefined}
              aria-describedby={described(`${id}-${key}`, wrong)}
              type="number"
              min={1}
              max={LIMIT_MAX[key]}
              className="w-36"
              disabled={disabled}
              title={disabled ? t("mqttbrokers.edit.readOnly") : undefined}
              value={value ?? LIMIT_DEFAULTS[key]}
              onChange={(event) => setLimit(key, wholeNumber(event.target.value))}
            />
            <span className="text-caption text-fg-muted">{t(`mqttbrokers.edit.unit.${key}`)}</span>
          </div>
          {use[key] !== undefined ? (
            <>
              <meter
                className="h-2 w-full max-w-xs"
                min={0}
                max={limits[key]}
                value={Math.min(use[key] ?? 0, limits[key])}
                aria-label={t("mqttbrokers.page.use", { use: number.format(use[key] ?? 0), limit: number.format(limits[key]) })}
              />
              <span className="text-caption text-fg-muted">
                {t("mqttbrokers.page.use", { use: number.format(use[key] ?? 0), limit: number.format(limits[key]) })}
              </span>
            </>
          ) : null}
        </div>
      </Field>
    );
  };
  return (
    <div className="flex flex-col gap-3">
      {field("messagesPerSecond")}
      {field("maxConnections")}
      {field("maxMessageBytes")}
      <Card padding="sm">
      <details>
        <summary className="focus-ring cursor-pointer font-medium text-fg">{t("mqttbrokers.edit.advanced")}</summary>
        <div className="mt-3 flex flex-col gap-3">
          {field("connectionsPerAddress")}
          <Field
            id={`${id}-port`}
            label={t("mqttbrokers.field.port")}
            help={t("mqttbrokers.edit.hint.portHint", { min: DEDICATED_PORTS.min, max: DEDICATED_PORTS.max })}
            errors={
              draft.form.port !== undefined && (draft.form.port < DEDICATED_PORTS.min || draft.form.port > DEDICATED_PORTS.max)
                ? [t("mqttbrokers.edit.portOutside", { min: DEDICATED_PORTS.min, max: DEDICATED_PORTS.max })]
                : undefined
            }
          >
            <Input
              id={`${id}-port`}
              type="number"
              className="w-36"
              min={DEDICATED_PORTS.min}
              max={DEDICATED_PORTS.max}
              disabled={disabled}
              title={disabled ? t("mqttbrokers.edit.readOnly") : undefined}
              value={draft.form.port ?? ""}
              placeholder="8883"
              onChange={(event) => onChange({ port: wholeNumber(event.target.value) })}
            />
          </Field>
        </div>
      </details>
      </Card>
    </div>
  );
}

function Networks({ sources, disabled, onChange }: { sources: string[]; disabled: boolean; onChange: (sources: string[]) => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-3" data-testid="broker-sources">
      {sources.length === 0 ? (
        <Alert tone="warning" title={t("mqttbrokers.connect.anySource")}>
          {t(disabled ? "mqttbrokers.edit.anySourceReadOnly" : "mqttbrokers.page.anySourceHint")}
        </Alert>
      ) : null}
      <ChipInput
        label={t("mqttbrokers.edit.sources")}
        placeholder={t("mqttbrokers.edit.hint.sourceExample")}
        help={t("mqttbrokers.edit.hint.sourcesHint")}
        values={sources}
        disabled={disabled}
        problem={(value) => {
          const wrong = sourceProblem(value, sources.filter((s) => s !== value));
          return wrong ? `source.${wrong}` : undefined;
        }}
        onChange={onChange}
      />
    </div>
  );
}

/** The help and error ids a Field gives `id`, for a control it cannot wire itself (it wraps a row). */
function described(id: string, wrong: boolean): string {
  const ids = fieldIds(id);
  return wrong ? `${ids.help} ${ids.error}` : ids.help;
}

/** A number field's text as a whole number, or undefined when it is empty. */
function wholeNumber(text: string): number | undefined {
  if (text.trim() === "") return undefined;
  const value = Number(text);
  return Number.isFinite(value) ? Math.trunc(value) : undefined;
}
