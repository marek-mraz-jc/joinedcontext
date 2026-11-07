import { useEffect, useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { asManifests } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useReadableList } from "../../api/permissions";
import type { components } from "../../api/schema";
import { Alert, Badge, Button, Dialog, Field, Input, Select, Switch, Textarea } from "../../components/ui";
import type { BadgeTone } from "../../components/ui";
import { holdersOf, rolesHeldBy } from "./brokerDraft";

type Role = { name?: string; serviceAccounts?: string[]; publish?: string[] };
type DryRun = components["schemas"]["DryRun"];
type Step = components["schemas"]["Step"];
type Watched = components["schemas"]["Watched"];

/** How long the watch polls after it is switched on, and how often (T-5534). */
export const WATCH_MS = 60_000;
export const WATCH_EVERY_MS = 2_000;

const TONE: Record<Step["outcome"], BadgeTone> = { ok: "success", refused: "danger", unchecked: "neutral" };

/** The accounts holding a role that may publish, sorted, once each: the only ones a device test makes sense for. */
export function publishers(roles: Role[], accounts: Manifest[], broker: string, project: string): string[] {
  const publishing = roles.filter((role) => (role.publish ?? []).length > 0);
  return [...new Set(publishing.flatMap((role) => holdersOf(role, accounts, broker, project).map((holder) => holder.account)))].sort();
}

/** A topic the account's first publish filter allows, `${clientid}` bound and each wildcard given a sample level. */
export function sampleTopic(roles: Role[], accounts: Manifest[], broker: string, project: string, account: string): string {
  const filter = rolesHeldBy(account, roles, accounts, broker, project).find((role) => (role.publish ?? []).length > 0)?.publish?.[0];
  if (!filter) return "";
  return filter
    .replaceAll("${clientid}", `${project}-${account}`)
    .replace(/\/#$/, "/reading")
    .replace(/^#$/, "reading")
    .split("/")
    .map((level) => (level === "+" ? "sensor-1" : level))
    .join("/");
}

/** The dry run of one device message and a watch for the real one (API/01 §44a). */
export function TestPublishDialog({
  project,
  broker,
  roles,
  open,
  onOpenChange,
}: {
  project: string;
  broker: string;
  roles: Role[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const readable = asManifests(useReadableList(project, "serviceaccounts", "ServiceAccount").data?.items ?? []);
  const accounts = publishers(roles, readable, broker, project);
  // The accounts arrive with their list: until one is picked, the first that publishes is.
  const [picked, setAccount] = useState("");
  const account = accounts.includes(picked) ? picked : (accounts[0] ?? "");
  const [typed, setTopic] = useState<string | null>(null);
  const topic = typed ?? sampleTopic(roles, readable, broker, project, account);
  const [payload, setPayload] = useState('{"temperature": 21.5}');
  const run = useMutation({
    mutationFn: async (): Promise<DryRun> =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/mqttbrokers/{name}/test-publish", {
          params: { path: { project, name: broker } },
          body: { account, topic, payload },
        }),
      ),
  });
  const changeAccount = (next: string) => {
    setAccount(next);
    setTopic(null);
    run.reset();
  };
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("mqttbrokers.test.title", { broker })}
      description={t("mqttbrokers.test.description")}
      closeLabel={t("app.close")}
      size="lg"
    >
      {accounts.length === 0 ? (
        <Alert tone="info">{t("mqttbrokers.test.noPublisher")}</Alert>
      ) : (
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            run.mutate();
          }}
        >
          <Field id={`${id}-account`} label={t("mqttbrokers.test.account")} help={t("mqttbrokers.test.hints.account")} required>
            <Select id={`${id}-account`} value={account} onChange={(event) => changeAccount(event.target.value)}>
              {accounts.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          </Field>
          <Field id={`${id}-topic`} label={t("mqttbrokers.test.topic")} help={t("mqttbrokers.test.hints.topic")} required>
            <Input id={`${id}-topic`} value={topic} onChange={(event) => setTopic(event.target.value)} className="font-mono" />
          </Field>
          <Field id={`${id}-payload`} label={t("mqttbrokers.test.payload")} help={t("mqttbrokers.test.hints.payload")}>
            <Textarea id={`${id}-payload`} value={payload} rows={4} onChange={(event) => setPayload(event.target.value)} className="font-mono" />
          </Field>
          <div>
            <Button type="submit" variant="primary" loading={run.isPending}>
              {t("mqttbrokers.test.run")}
            </Button>
          </div>
          {run.isError ? <Alert tone="danger">{run.error instanceof Error ? run.error.message : t("mqttbrokers.test.failed")}</Alert> : null}
          {run.data ? <Steps run={run.data} /> : null}
          <Watch project={project} broker={broker} account={account} />
        </form>
      )}
    </Dialog>
  );
}

function Steps({ run }: { run: DryRun }): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-label={t("mqttbrokers.test.result")} className="flex flex-col gap-2">
      <p className="text-body text-fg-muted">{t("mqttbrokers.test.nothingSent", { client: run.clientId })}</p>
      <ol className="flex flex-col gap-2">
        {run.steps.map((step) => (
          <li key={step.step} className="flex flex-wrap items-baseline gap-2" data-step={step.step} data-outcome={step.outcome}>
            <Badge tone={TONE[step.outcome]}>{t(`mqttbrokers.test.outcome.${step.outcome}`)}</Badge>
            <span className="font-semibold">{t(`mqttbrokers.test.step.${step.step}`, { defaultValue: step.step })}</span>
            <span className="min-w-0 break-words text-fg-muted">{step.reason}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}

/** Polls the watch every two seconds for a minute once switched on, from the moment it was switched on. */
function Watch({ project, broker, account }: { project: string; broker: string; account: string }): JSX.Element {
  const { t } = useTranslation();
  const [since, setSince] = useState<string | null>(null);
  useEffect(() => {
    if (!since) return;
    const stop = setTimeout(() => setSince(null), WATCH_MS);
    return () => clearTimeout(stop);
  }, [since]);
  const watched = useQuery({
    queryKey: ["mqttBrokerWatch", project, broker, account, since],
    enabled: since !== null,
    refetchInterval: WATCH_EVERY_MS,
    queryFn: async (): Promise<Watched> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/mqttbrokers/{name}/watch", {
          params: { path: { project, name: broker }, query: { account, since: since ?? undefined } },
        }),
      ),
  });
  return (
    <section className="flex flex-col gap-2 border-t border-border pt-3">
      <Switch
        checked={since !== null}
        onCheckedChange={(on) => setSince(on ? new Date().toISOString() : null)}
        label={t("mqttbrokers.test.watch")}
      />
      <p className="text-caption text-fg-muted">{t("mqttbrokers.test.hints.watch")}</p>
      {since && watched.data ? <Watching watched={watched.data} /> : null}
    </section>
  );
}

export function Watching({ watched }: { watched: Watched }): JSX.Element {
  const { t } = useTranslation();
  const time = new Intl.DateTimeFormat(undefined, { timeStyle: "medium" });
  return (
    <div className="flex flex-col gap-2" aria-live="polite">
      {watched.connected != null ? (
        <p data-connected={watched.connected}>{t("mqttbrokers.test.connected", { count: watched.connected })}</p>
      ) : null}
      {watched.refusals.length === 0 ? (
        <p className="text-fg-muted">{watched.refusalsRead ? t("mqttbrokers.test.noRefusal") : t("mqttbrokers.test.refusalsUnread")}</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {watched.refusals.map((refusal) => (
            <li key={`${refusal.at}-${refusal.reason}`} className="flex flex-wrap items-baseline gap-2" data-refusal={refusal.reason}>
              <Badge tone="danger">{t("mqttbrokers.test.outcome.refused")}</Badge>
              <span>{time.format(new Date(refusal.at))}</span>
              <span>{t(`mqttbrokers.test.refusal.${refusal.reason}`, { defaultValue: refusal.reason })}</span>
            </li>
          ))}
        </ul>
      )}
      {watched.reason ? <p className="text-caption text-fg-muted">{watched.reason}</p> : null}
    </div>
  );
}
