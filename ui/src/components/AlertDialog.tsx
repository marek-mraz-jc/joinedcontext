/**
 * "Alert me" on a pipeline or a space (API/01 §37, PL-71, T-3261): which of failure, stale data,
 * zero output and a newly broken model rule (T-5066) to be told about, now or as a daily digest, muted for a while or stopped. E-mail is
 * shown and refused with its reason: the Portal has no mail relay yet. On an agent profile
 * (T-5301) the events are its refusal spikes, runs near the token cap and off-hours writes, in a
 * project the person approves in, picked from `projects`.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../api/client";
import type { components } from "../api/schema";
import { Alert, Button, Checkbox, Dialog, Field, RadioGroup, Select } from "./ui";

type Subscription = components["schemas"]["AlertSubscription"];
type Event = Subscription["events"][number];
const PIPELINE_EVENTS: Event[] = ["failure", "stale", "zero", "invalid"];
const AGENT_EVENTS: Event[] = ["refusalSpike", "spendShare", "offHoursWrite"];
export const ALERTS_KEY = ["alerts"];

export interface AlertDialogProps {
  project: string;
  scope: Subscription["scope"];
  target: string;
  /** What the dialog names in its title: the pipeline, the space, the type or the agent. */
  label: string;
  /** The projects the person may choose between, as the server offered them; `project` first. */
  projects?: string[];
  onClose: () => void;
}

export function AlertDialog({ project: offered, scope, target, label, projects, onClose }: AlertDialogProps): JSX.Element {
  const EVENTS = scope === "agent" ? AGENT_EVENTS : PIPELINE_EVENTS;
  const [project, setProject] = useState(offered);
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const mine = useQuery({
    queryKey: ALERTS_KEY,
    queryFn: async () => (await unwrap(await api.GET("/api/v1/alerts", {}))).items,
  });
  const current = mine.data?.find((one) => one.project === project && one.scope === scope && one.target === target);
  const [events, setEvents] = useState<Event[] | null>(null);
  const [delivery, setDelivery] = useState<"portal" | "digest" | "email" | null>(null);
  const shownEvents = events ?? current?.events ?? EVENTS;
  const shownDelivery = delivery ?? current?.delivery ?? "portal";
  const [problem, setProblem] = useState<string | null>(null);
  const done = async () => {
    setProblem(null);
    await queryClient.invalidateQueries({ queryKey: ALERTS_KEY });
  };
  const fail = (error: unknown) => setProblem(error instanceof Error ? error.message : String(error));

  const save = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.PUT("/api/v1/alerts", {
          body: { project, scope, target, events: shownEvents, delivery: shownDelivery },
        }),
      ),
    onSuccess: done,
    onError: fail,
  });
  const stop = useMutation({
    mutationFn: async (id: number) => {
      await unwrap(await api.DELETE("/api/v1/alerts/{id}", { params: { path: { id } } }));
    },
    onSuccess: done,
    onError: fail,
  });
  const mute = useMutation({
    mutationFn: async ({ id, duration }: { id: number; duration: string | null }) =>
      unwrap(await api.POST("/api/v1/alerts/{id}/mute", { params: { path: { id } }, body: { for: duration } })),
    onSuccess: done,
    onError: fail,
  });
  const until = current?.mutedUntil
    ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(current.mutedUntil))
    : undefined;

  return (
    <Dialog
      open
      onOpenChange={(open) => (open ? undefined : onClose())}
      title={t("alerts.title", { name: label })}
      description={t(`alerts.lead.${scope}`)}
      closeLabel={t("alerts.close")}
    >
      <div className="flex flex-col gap-3">
        {projects && projects.length > 1 ? (
          <Field id="alert-project" label={t("alerts.project")} help={t("alerts.projectHelp")}>
            <Select
              id="alert-project"
              value={project}
              onChange={(event) => {
                setProject(event.target.value);
                setEvents(null);
                setDelivery(null);
              }}
            >
              {projects.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
        <fieldset className="flex flex-col gap-1">
          <legend className="text-body font-medium text-fg">{t("alerts.events")}</legend>
          {EVENTS.map((event) => (
            <Checkbox
              key={event}
              label={t(`alerts.event.${event}`)}
              checked={shownEvents.includes(event)}
              onChange={(e) =>
                setEvents(e.target.checked ? [...shownEvents, event] : shownEvents.filter((one) => one !== event))
              }
            />
          ))}
        </fieldset>
        <RadioGroup
          name="alert-delivery"
          legend={t("alerts.delivery")}
          value={shownDelivery}
          onChange={setDelivery}
          options={[
            { value: "portal", label: t("alerts.portal"), description: t("alerts.portalHint") },
            { value: "digest", label: t("alerts.digest"), description: t("alerts.digestHint") },
            { value: "email", label: t("alerts.email"), description: t("alerts.emailHint"), disabled: true },
          ]}
        />
        {problem ? (
          <Alert role="alert" tone="danger">
            {problem}
          </Alert>
        ) : null}
        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant="primary"
            loading={save.isPending}
            disabled={shownEvents.length === 0}
            disabledReason={shownEvents.length === 0 ? t("alerts.noEvent") : undefined}
            onClick={() => save.mutate()}
          >
            {current ? t("alerts.change") : t("alerts.subscribe")}
          </Button>
          {current ? (
            <Button variant="ghost" loading={stop.isPending} onClick={() => stop.mutate(current.id)}>
              {t("alerts.stop")}
            </Button>
          ) : null}
        </div>
        {current ? (
          <div className="flex flex-wrap items-end gap-2 border-t border-border pt-3">
            <p role="status" className="w-full text-caption text-fg-muted">
              {until ? t("alerts.mutedUntil", { until }) : t("alerts.active")}
            </p>
            <label className="flex flex-col gap-1 text-caption text-fg-muted">
              {t("alerts.mute")}
              <Select
                value=""
                onChange={(event) => {
                  const value = event.target.value;
                  if (value) mute.mutate({ id: current.id, duration: value === "none" ? null : value });
                }}
              >
                <option value="">{t("alerts.muteChoose")}</option>
                {["1h", "1d", "7d", "forever"].map((duration) => (
                  <option key={duration} value={duration}>
                    {t(`alerts.muteFor.${duration}`)}
                  </option>
                ))}
                {until ? <option value="none">{t("alerts.unmute")}</option> : null}
              </Select>
            </label>
          </div>
        ) : null}
      </div>
    </Dialog>
  );
}
