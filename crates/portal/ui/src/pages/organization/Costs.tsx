import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap, reasonOf } from "../../api/client";
import { ORG_NAMESPACE } from "../../api/manifest";
import { administersOrganization, usePermissions } from "../../api/permissions";
import type { components } from "../../api/schema";
import { Alert, Button, EmptyState, Field, Input } from "../../components/ui";
import { AdminGrid } from "../../components/grid/AdminGrid";
import type { DataColumn } from "../../components/grid/AdminGrid";
import { download } from "../models/diagramExport";

type ProjectCost = components["schemas"]["ProjectCost"];

/** One line of the table: a project, or the platform's own share (no project, no AI). */
interface CostRow {
  key: string;
  project: ProjectCost | null;
  node: number;
  storage: number;
  ai: number | null;
  total: number;
}

/** The month before `now`'s as `2026-09`: the last one billed whole, which the page opens on. */
export function lastMonth(now: Date): string {
  const year = now.getUTCMonth() === 0 ? now.getUTCFullYear() - 1 : now.getUTCFullYear();
  const month = now.getUTCMonth() === 0 ? 12 : now.getUTCMonth();
  return `${year}-${String(month).padStart(2, "0")}`;
}

/** The running month as `2026-10`: the latest the server prices. */
function thisMonth(now: Date): string {
  return `${now.getUTCFullYear()}-${String(now.getUTCMonth() + 1).padStart(2, "0")}`;
}

/**
 * Administration → Costs (T-5126): one month of the installation split per project, the node
 * share, storage and AI apart, with each space and assistant under its project, and the same
 * month as CSV. Only an administrator of the organization reads it; anyone else is told so and
 * nothing is fetched.
 */
export function Costs(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const permissions = usePermissions(ORG_NAMESPACE);
  const administers = administersOrganization(permissions.data);
  const now = new Date();
  const [month, setMonth] = useState(lastMonth(now));
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const costs = useQuery({
    queryKey: ["organizationCosts", month],
    enabled: administers && month !== "",
    queryFn: async () => unwrap(await api.GET("/api/v1/organization/costs", { params: { query: { month } } })),
  });

  if (permissions.isLoading) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  const data = costs.data;
  const money = new Intl.NumberFormat(locale, { style: "currency", currency: data?.currency ?? "EUR" });
  const bytes = new Intl.NumberFormat(locale, { style: "unit", unit: "gigabyte", maximumFractionDigits: 2 });
  const usd = new Intl.NumberFormat(locale, { style: "currency", currency: "USD", maximumFractionDigits: 4 });
  const size = (n: number | null | undefined) => (n === null || n === undefined ? "—" : bytes.format(n / 1e9));
  const partial = (data?.projects ?? []).some(
    (p) => p.spaces.some((s) => s.reason) || p.assistants.some((a) => a.reason),
  );

  const rows: CostRow[] = data
    ? [
        ...data.projects.map((p) => ({
          key: `p-${p.project}`,
          project: p,
          node: p.node,
          storage: p.storage,
          ai: p.ai,
          total: p.total,
        })),
        ...(data.projects.length === 0
          ? []
          : [
              {
                key: "platform",
                project: null,
                node: data.platform.node,
                storage: data.platform.storage,
                ai: null,
                total: data.platform.total,
              },
            ]),
      ]
    : [];
  const amount = (value: number | null) => (value === null ? "" : String(value));
  const columns: DataColumn<CostRow>[] = [
    {
      key: "project",
      label: t("organization.costs.project"),
      text: (row) => (row.project ? row.project.project : t("organization.costs.platform")),
      cell: (row) =>
        row.project ? (
          <>
            <span className="font-mono">{row.project.project}</span>
            {details(row.project)}
          </>
        ) : (
          <>
            {t("organization.costs.platform")}
            <span className="block text-caption text-fg-muted">{t("organization.costs.platformHint")}</span>
          </>
        ),
    },
    {
      key: "node",
      label: t("organization.costs.node"),
      kind: "number",
      align: "right",
      text: (row) => amount(row.node),
      cell: (row) => money.format(row.node),
    },
    {
      key: "storage",
      label: t("organization.costs.storage"),
      kind: "number",
      align: "right",
      text: (row) => amount(row.storage),
      cell: (row) => money.format(row.storage),
    },
    {
      key: "ai",
      label: t("organization.costs.ai"),
      kind: "number",
      align: "right",
      text: (row) => amount(row.ai),
      cell: (row) => (row.ai === null ? "—" : money.format(row.ai)),
    },
    {
      key: "total",
      label: t("organization.costs.total"),
      kind: "number",
      align: "right",
      text: (row) => amount(row.total),
      cell: (row) => money.format(row.total),
    },
  ];

  const save = async () => {
    if (saving) return;
    setSaving(true);
    setFailed(null);
    try {
      const csv: unknown = await unwrap(
        await api.GET("/api/v1/organization/costs/export", { params: { query: { month } }, parseAs: "text" }),
      );
      download(typeof csv === "string" ? csv : "", `costs-${month}.csv`, "text/csv;charset=utf-8");
    } catch (error) {
      setFailed(reasonOf(error, t("app.error.generic")));
    } finally {
      setSaving(false);
    }
  };

  const details = (p: ProjectCost) => (
    <details>
      <summary className="cursor-pointer text-caption text-fg-muted">
        {t("organization.costs.parts", { spaces: p.spaces.length, assistants: p.assistants.length })}
      </summary>
      <ul className="mt-1 space-y-1 text-caption">
        {p.spaces.map((s) => (
          <li key={`s-${s.space}`}>
            {t("organization.costs.spaceLine", { space: s.space, size: size(s.bytes), cost: money.format(s.storage) })}
            {s.reason ? <span className="block text-fg-muted">{s.reason}</span> : null}
          </li>
        ))}
        {p.assistants.map((a) => (
          <li key={`a-${a.assistant}`}>
            {t("organization.costs.assistantLine", {
              assistant: a.assistant,
              questions: a.questions,
              usd: a.costUsd === null || a.costUsd === undefined ? "—" : usd.format(a.costUsd),
              cost: money.format(a.cost),
            })}
            {a.uncostedQuestions > 0 ? (
              <span className="block text-fg-muted">{t("organization.costs.uncosted", { count: a.uncostedQuestions })}</span>
            ) : null}
            {a.reason ? <span className="block text-fg-muted">{a.reason}</span> : null}
          </li>
        ))}
      </ul>
    </details>
  );

  return (
    <section className="space-y-4" aria-labelledby="organization-costs-heading">
      <div>
        <h2 id="organization-costs-heading" className="text-title font-semibold text-fg">
          {t("organization.costs.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("organization.costs.lead")}</p>
      </div>
      {!administers ? (
        <Alert tone="info">{t("organization.costs.hidden")}</Alert>
      ) : (
        <>
          <div className="flex flex-wrap items-end gap-3">
            <Field id="costs-month" label={t("organization.costs.month")} help={t("organization.costs.monthHelp")}>
              <Input
                id="costs-month"
                type="month"
                max={thisMonth(now)}
                value={month}
                onChange={(event) => setMonth(event.target.value)}
              />
            </Field>
            {costs.isSuccess ? (
              <Button type="button" variant="secondary" onClick={() => void save()} aria-busy={saving}>
                {saving ? t("organization.costs.saving") : t("organization.costs.export")}
              </Button>
            ) : null}
          </div>
          {failed ? <Alert tone="danger">{failed}</Alert> : null}
          {data ? (
            <p className="text-body text-fg">
              {t("organization.costs.summary", {
                infrastructure: money.format(data.infrastructure),
                ai: money.format(data.ai),
                total: money.format(data.total),
              })}
              <span className="block text-caption text-fg-muted">
                {t("organization.costs.measured", {
                  at: new Date(data.measuredAt).toLocaleString(locale),
                  rate: new Intl.NumberFormat(locale, { maximumFractionDigits: 4 }).format(data.prices.perUsd),
                  currency: data.currency,
                })}
              </span>
            </p>
          ) : null}
          {partial ? (
            <Alert tone="warning" role="status">
              {t("organization.costs.partial")}
            </Alert>
          ) : null}
          <AdminGrid<CostRow>
            query={costs}
            caption={t("organization.costs.caption", { month })}
            storageKey="organization:costs"
            rows={rows}
            rowKey={(row) => row.key}
            columns={columns}
            stack
            empty={<EmptyState bare title={t("organization.costs.empty")} />}
          />
        </>
      )}
    </section>
  );
}
