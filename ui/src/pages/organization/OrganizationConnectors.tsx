import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { ORG_NAMESPACE } from "../../api/manifest";
import { administersOrganization, usePermissions } from "../../api/permissions";
import { personLabel, usePersonNames } from "../../api/personNames";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  ConfirmDialog,
  EmptyState,
  Field,
  Input,
  Select,
} from "../../components/ui";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import type { BadgeTone } from "../../components/ui/Badge";
import { PageFailed } from "../../components/ui/PageState";

type Usage = components["schemas"]["ConnectorUsage"];

const STATES = ["active", "expired", "revoked"] as const;
type State = (typeof STATES)[number];
const TONE: Record<State, BadgeTone> = { active: "success", expired: "neutral", revoked: "neutral" };

const QUERY = ["organizationConnectors"] as const;

/** Whether a row matches the filter text: its name, owner, owner's name or an allowed client. */
function matches(row: Usage, text: string, names: Map<string, string>): boolean {
  const wanted = text.trim().toLowerCase();
  if (wanted === "") return true;
  return [row.name, row.owner, names.get(row.owner) ?? "", ...row.allowedClients].some((value) =>
    value.toLowerCase().includes(wanted),
  );
}

/**
 * Administration → Connectors (T-5975): every person's connectors, with their scope, clients and
 * the last day's calls and denials, a flag on a connector refused often, and a revoke. It shows
 * what a connector may reach and how much it is used, never what a call carried or a token. Only
 * an administrator of the organization reads it; the server refuses anyone else, and the page
 * fetches nothing for them.
 */
export function OrganizationConnectors(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const queryClient = useQueryClient();
  const permissions = usePermissions(ORG_NAMESPACE);
  const administers = administersOrganization(permissions.data);
  const overview = useQuery({
    queryKey: QUERY,
    enabled: administers,
    queryFn: async () => unwrap(await api.GET("/api/v1/organization/connectors")),
  });
  const [text, setText] = useState("");
  const [state, setState] = useState<State | "">("active");
  const [flaggedOnly, setFlaggedOnly] = useState(false);
  const [revoking, setRevoking] = useState<Usage | null>(null);
  const revoke = useMutation({
    mutationFn: async (id: string) =>
      unwrap(await api.DELETE("/api/v1/organization/connectors/{id}", { params: { path: { id } } })),
    onSuccess: async () => {
      setRevoking(null);
      await queryClient.invalidateQueries({ queryKey: QUERY });
    },
  });

  const items = overview.data?.items ?? [];
  const names = usePersonNames(items.map((row) => row.owner));
  const shown = items.filter(
    (row) => (state === "" || row.state === state) && (!flaggedOnly || row.flagged) && matches(row, text, names),
  );
  const flagged = items.filter((row) => row.flagged && row.state === "active").length;
  const when = (at: string) =>
    new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(at));

  if (permissions.isLoading) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }

  return (
    <section className="space-y-4" aria-labelledby="organization-connectors-heading">
      <div>
        <h2 id="organization-connectors-heading" className="text-title font-semibold text-fg">
          {t("organization.connectors.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("organization.connectors.lead")}</p>
      </div>
      {!administers ? (
        <Alert tone="info">{t("organization.connectors.hidden")}</Alert>
      ) : overview.isError ? (
        <PageFailed error={overview.error} onRetry={() => void overview.refetch()} />
      ) : (
        <>
          {flagged > 0 ? (
            <Alert tone="warning" role="status">
              {t("organization.connectors.flaggedSummary", {
                count: flagged,
                denials: overview.data?.denialsFlag ?? 0,
              })}
            </Alert>
          ) : null}
          {revoke.isError ? <Alert tone="danger">{revoke.error.message}</Alert> : null}
          <div className="grid gap-4 sm:grid-cols-[minmax(0,2fr)_minmax(0,1fr)_auto] sm:items-start">
            <Field
              id="connectors-filter"
              label={t("organization.connectors.filter")}
              help={t("organization.connectors.filterHelp")}
            >
              <Input id="connectors-filter" type="search" value={text} onChange={(e) => setText(e.target.value)} />
            </Field>
            <Field
              id="connectors-state"
              label={t("organization.connectors.state")}
              help={t("organization.connectors.stateHelp")}
            >
              <Select id="connectors-state" value={state} onChange={(e) => setState(e.target.value as State | "")}>
                <option value="">{t("organization.connectors.stateAny")}</option>
                {STATES.map((value) => (
                  <option key={value} value={value}>
                    {t(`organization.connectors.states.${value}`)}
                  </option>
                ))}
              </Select>
            </Field>
            <Checkbox
              id="connectors-flagged"
              className="sm:pt-8"
              label={t("organization.connectors.flaggedOnly")}
              hint={t("organization.connectors.flaggedHelp")}
              checked={flaggedOnly}
              onChange={(e) => setFlaggedOnly(e.target.checked)}
            />
          </div>
          <ManifestGrid<Usage>
            caption={t("organization.connectors.caption")}
            storageKey="organization-connectors"
            searchLabel={t("organization.connectors.filter")}
            search={text}
            onSearchChange={setText}
            total={items.length}
            rows={shown}
            rowKey={(row) => row.id}
            query={overview}
            name={{
              id: "name",
              label: t("organization.connectors.connector"),
              text: (row) => row.name,
              cell: (row) => (
                <>
                  <span className="font-medium">{row.name}</span>
                  <span className="block">
                    <Badge tone={TONE[row.state as State] ?? "neutral"}>{t(`organization.connectors.states.${row.state}`)}</Badge>
                    {row.writes ? (
                      <Badge tone="warning" className="ml-1">
                        {t("organization.connectors.writes")}
                      </Badge>
                    ) : null}
                  </span>
                </>
              ),
            }}
            columns={[
              { id: "owner", label: t("organization.connectors.owner"), filter: true, text: (row) => personLabel(row.owner, names) },
              {
                id: "scope",
                label: t("organization.connectors.scope"),
                text: (row) => row.segments.join(", "),
                cell: (row) => (
                  <>
                    <span className="block">{row.segments.join(", ")}</span>
                    <span className="block text-caption text-fg-muted">
                      {row.scope.map((item) => [item.project, item.space ?? item.endpoint ?? item.pipeline].filter(Boolean).join("/")).join(", ")}
                    </span>
                  </>
                ),
              },
              {
                id: "clients",
                label: t("organization.connectors.clients"),
                text: (row) => (row.allowedClients.length === 0 ? t("organization.connectors.anyClient") : row.allowedClients.join(", ")),
              },
              {
                id: "lastCall",
                label: t("organization.connectors.lastCall"),
                text: (row) => row.lastUsedAt ?? "",
                cell: (row) => (row.lastUsedAt ? <time dateTime={row.lastUsedAt}>{when(row.lastUsedAt)}</time> : t("organization.connectors.never")),
              },
              {
                id: "calls",
                label: t("organization.connectors.callsPerDay"),
                numeric: true,
                align: "right",
                text: (row) => String(row.callsPerDay),
                cell: (row) => t("organization.connectors.calls", { count: row.callsPerDay }),
              },
              {
                id: "denials",
                label: t("organization.connectors.denialsPerDay"),
                numeric: true,
                align: "right",
                text: (row) => String(row.denialsPerDay),
                cell: (row) => (
                  <>
                    <span className="block">{t("organization.connectors.denials", { count: row.denialsPerDay })}</span>
                    {row.flagged ? <Badge tone="danger">{t("organization.connectors.flagged")}</Badge> : null}
                  </>
                ),
              },
            ]}
            actions={(row) =>
              row.state === "active" ? (
                <Button
                  size="sm"
                  variant="secondary"
                  aria-label={t("organization.connectors.revokeLabel", { name: row.name })}
                  onClick={() => setRevoking(row)}
                >
                  {t("organization.connectors.revoke")}
                </Button>
              ) : null
            }
            empty={
              <EmptyState
                bare
                title={t(items.length === 0 ? "organization.connectors.empty" : "organization.connectors.noMatch")}
                description={t(items.length === 0 ? "organization.connectors.emptyHint" : "organization.connectors.noMatchHint")}
              />
            }
          />
          <ConfirmDialog
            open={revoking !== null}
            onOpenChange={(open) => (open ? undefined : setRevoking(null))}
            title={t("organization.connectors.confirmTitle", { name: revoking?.name ?? "" })}
            description={t("organization.connectors.confirmText", {
              owner: revoking ? personLabel(revoking.owner, names) : "",
            })}
            confirmLabel={t("organization.connectors.revoke")}
            tone="danger"
            pending={revoke.isPending}
            onConfirm={() => (revoking ? revoke.mutate(revoking.id) : undefined)}
          />
        </>
      )}
    </section>
  );
}
