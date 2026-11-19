import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import type { components } from "../../api/schema";
import { Badge, Card, Checkbox, Icon } from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";
import { readableQuery } from "./queries";

type ReadableDataset = components["schemas"]["ReadableDataset"];

const TONE: Record<string, BadgeTone> = { public: "success", organization: "primary", group: "accent", granted: "info" };

/**
 * What grants a dataset, in words: public, your organization, the group, or you alone. `words`
 * names the locale group, so an administrator's view of another person says "everyone in the
 * organization" instead of "your organization".
 */
export function AccessBadge({
  dataset,
  words = "catalogue.readable.access",
}: {
  dataset: Pick<ReadableDataset, "access" | "group">;
  words?: "catalogue.readable.access" | "dataAccess.personAccess";
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Badge tone={TONE[dataset.access] ?? "neutral"}>
      {t(`${words}.${dataset.access}`, { group: dataset.group ?? "" })}
    </Badge>
  );
}

export type GrantedState = "off" | "loading" | "failed" | "noProject" | "ready";

export interface Granted {
  state: GrantedState;
  /** The internal datasets granted to the person that match the search words. */
  datasets: ReadableDataset[];
  /** Endpoints the server could not ask in time, so their datasets are missing. */
  unanswered: number;
}

/**
 * The internal datasets a signed-in person may read (ADR-N-067 §2.4, T-6411): the server asks the
 * gateway per Endpoint and lists only what it grants. The page shows them as a filter of its one
 * list, never as a box of their own above it (T-7389); the search words narrow them by name.
 */
export function useGranted(signedIn: boolean, q: string | undefined): Granted {
  const readable = useQuery({ ...readableQuery(), enabled: signedIn });
  if (!signedIn) {
    return { state: "off", datasets: [], unanswered: 0 };
  }
  if (readable.isPending) {
    return { state: "loading", datasets: [], unanswered: 0 };
  }
  // On an Organization's own host the Portal refuses everyone who belongs to none of its
  // projects (403): a newcomer's normal state, said as one, not a failure (T-7317).
  if (readable.error instanceof ApiError && readable.error.status === 403) {
    return { state: "noProject", datasets: [], unanswered: 0 };
  }
  if (readable.isError || !Array.isArray(readable.data.datasets)) {
    return { state: "failed", datasets: [], unanswered: 0 };
  }
  const words = q?.trim().toLocaleLowerCase() ?? "";
  const datasets = readable.data.datasets
    .filter((dataset) => dataset.access !== "public")
    .filter((dataset) =>
      [dataset.title, dataset.name, dataset.description].some((text) => text?.toLocaleLowerCase().includes(words)),
    );
  return { state: "ready", datasets, unanswered: readable.data.unanswered };
}

/**
 * The Access filter at the top of the sidebar (T-7389): the datasets granted to the person, or
 * the public ones. With nothing granted the first option stays in place, refused, and says why.
 */
export function AccessFacet({
  granted,
  publicCount,
  selected,
  onSelect,
}: {
  granted: Granted;
  publicCount: number;
  selected: "granted" | "public" | undefined;
  onSelect: (access: "granted" | "public" | undefined) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const none = granted.datasets.length === 0;
  const why =
    granted.state === "loading"
      ? t("catalogue.access.loading")
      : granted.state === "noProject"
        ? t("catalogue.access.noProject")
        : granted.state === "failed"
          ? t("catalogue.access.failed")
          : t("catalogue.access.none");
  const option = (value: "granted" | "public", label: string, count: number, refused?: string) => (
    <Checkbox
      className="flex w-full items-start py-1.5 md:py-0 [&>span:first-of-type]:min-w-0 [&>span:first-of-type]:flex-1"
      checked={selected === value}
      disabled={refused !== undefined}
      disabledReason={refused}
      onChange={() => onSelect(selected === value ? undefined : value)}
      label={label}
      hint={<Badge tone="neutral" className="tabular-nums">{count}</Badge>}
    />
  );
  return (
    <Card flush>
      <fieldset className="flex flex-col gap-2 px-4 pb-4 pt-3">
        <legend className="sr-only">{t("catalogue.access.label")}</legend>
        <p aria-hidden="true" className="pb-1 text-body font-semibold text-fg">
          {t("catalogue.access.label")}
        </p>
        {option("granted", t("catalogue.access.granted"), granted.datasets.length, none && selected !== "granted" ? why : undefined)}
        {/* Seen by everyone; the refused box reads the same words to a screen reader itself. */}
        {none ? (
          <p aria-hidden="true" className="text-caption text-fg-muted">
            {why}
          </p>
        ) : null}
        {option("public", t("catalogue.access.public"), publicCount)}
      </fieldset>
    </Card>
  );
}

/** One granted dataset in the catalogue's list, marked "granted" and naming what grants it. */
export function GrantedCard({ dataset }: { dataset: ReadableDataset }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Card className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="min-w-0 break-words font-heading text-title font-semibold">
          <Link
            to="/catalogue/endpoint/$slug"
            params={{ slug: dataset.slug }}
            className="focus-ring rounded-sm text-fg hover:underline"
          >
            {dataset.title ?? dataset.name}
          </Link>
        </h2>
        <Badge tone="info">{t("catalogue.access.badge")}</Badge>
      </div>
      <p className="flex flex-wrap items-center gap-1 text-caption text-fg-muted">
        <Icon name="spaces" className="size-3.5" />
        {dataset.project} · {t(`catalogue.readable.access.${dataset.access}`, { group: dataset.group ?? "" })}
      </p>
      {dataset.description ? <p className="line-clamp-3 text-body text-fg">{dataset.description}</p> : null}
    </Card>
  );
}
