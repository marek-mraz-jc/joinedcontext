import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { doorAction, type RestAnswer } from "../../api/odata";
import { ApiError } from "../../api/client";
import type { components } from "../../api/schema";
import { Alert, Badge, Button, Card, ConfirmDialog, Field, Input, RadioGroup, Select } from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";

type PortalOrganization = components["schemas"]["PortalOrganization"];
type SiteHost = components["schemas"]["SiteHost"];

/** How a registry state reads, and its tone; only `served` answers (T-7415). */
const STATES: Record<string, BadgeTone> = {
  approved: "warning",
  "dns-ok": "info",
  "cert-issued": "info",
  served: "success",
};

/** The lines of a refusal: the server's per-field lines, else its detail. */
function linesOf(error: unknown, fallback: string): string[] {
  if (!(error instanceof ApiError)) return [fallback];
  const lines = error.problem?.errors?.length ? error.problem.errors : [error.problem?.detail ?? error.message];
  return lines.map((line) => line.replace(/^(host|organization): /, ""));
}

/** The host a draft names: `label.base` under an own host, or the whole name under the domain. */
export function hostOf(mode: "own" | "domain", label: string, base: string, whole: string): string {
  const name = mode === "own" ? `${label.trim()}.${base}` : whole.trim();
  return name.toLowerCase().replace(/\.$/, "");
}

/**
 * An Organization's own portal hosts (T-7415): the hosts its portals may answer on, each with its
 * way to `served`, and the form that adds one without an operator (one label under a host the
 * Organization serves, or a name under its verified domain). The server decides every add.
 */
export function PortalHostsPanel({ organization, showName }: { organization: PortalOrganization; showName: boolean }): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const bases = organization.bases;
  const verified = organization.verifiedDomain ?? undefined;
  const [mode, setMode] = useState<"own" | "domain">(bases.length > 0 ? "own" : "domain");
  const [label, setLabel] = useState("");
  const [base, setBase] = useState(bases[0] ?? "");
  const [whole, setWhole] = useState("");
  const [removing, setRemoving] = useState<string | undefined>();
  const add = useMutation({
    mutationFn: async () =>
      (await doorAction("portal_portal_host_add", {
        body: { organization: organization.organization, host: hostOf(mode, label, base, whole) },
      })) as RestAnswer<"add_portal_host">,
    onSuccess: async () => {
      setLabel("");
      setWhole("");
      await client.invalidateQueries({ queryKey: ["portalHosts"] });
    },
  });
  const remove = useMutation({
    mutationFn: async (host: string) => (await doorAction("portal_portal_host_remove", { host })) as RestAnswer<"remove_portal_host">,
    onSuccess: async () => {
      setRemoving(undefined);
      await client.invalidateQueries({ queryKey: ["portalHosts"] });
    },
  });
  const sites: SiteHost[] = organization.sites;
  const canAdd = bases.length > 0 || verified !== undefined;
  const ready = mode === "own" ? label.trim() !== "" && base !== "" : whole.trim() !== "";
  const errors = add.isError ? linesOf(add.error, t("app.error.generic")) : undefined;
  const titleId = `portal-hosts-${organization.organization}`;
  return (
    <Card className="space-y-4" aria-labelledby={titleId}>
      <div className="space-y-1">
        <h2 id={titleId} className="text-heading font-semibold">
          {showName ? t("portals.hosts.titleOf", { organization: organization.organization }) : t("portals.hosts.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("portals.hosts.description")}</p>
      </div>
      {sites.length === 0 ? (
        <p className="rounded-md border border-dashed border-border px-3 py-4 text-body text-fg-muted">{t("portals.hosts.empty")}</p>
      ) : (
        <ul className="divide-y divide-border rounded-md border border-border" aria-label={t("portals.hosts.title")}>
          {sites.map((site) => (
            <li key={site.host} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
              <span className="min-w-0 flex-1 break-all font-mono text-body">{site.host}</span>
              <Badge tone={STATES[site.state] ?? "neutral"}>{t(`portals.hosts.state.${site.state}`, { defaultValue: site.state })}</Badge>
              {site.removable ? (
                <Button variant="ghost" size="sm" onClick={() => setRemoving(site.host)} aria-label={t("portals.hosts.removeOf", { host: site.host })}>
                  {t("portals.hosts.remove")}
                </Button>
              ) : null}
              {site.detail ? <p className="basis-full text-caption text-fg-muted">{site.detail}</p> : null}
            </li>
          ))}
        </ul>
      )}
      {remove.isError ? (
        <Alert tone="danger" role="alert">
          {linesOf(remove.error, t("app.error.generic")).join(" ")}
        </Alert>
      ) : null}
      {canAdd ? (
        <form
          className="space-y-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (ready) add.mutate();
          }}
        >
          {bases.length > 0 && verified ? (
            <RadioGroup<"own" | "domain">
              name={`portal-host-mode-${organization.organization}`}
              legend={t("portals.hosts.mode")}
              value={mode}
              options={[
                { value: "own", label: t("portals.hosts.modeOwn") },
                { value: "domain", label: t("portals.hosts.modeDomain", { domain: verified }) },
              ]}
              onChange={setMode}
            />
          ) : null}
          {mode === "own" ? (
            <div className="flex flex-wrap items-start gap-2">
              <Field id={`portal-host-label-${organization.organization}`} label={t("portals.hosts.label")} description={t("portals.hosts.labelHint")} required errors={errors} className="min-w-0 flex-1 basis-40">
                <Input id={`portal-host-label-${organization.organization}`} value={label} placeholder="opendata2" autoComplete="off" onChange={(event) => setLabel(event.target.value)} />
              </Field>
              <Field id={`portal-host-base-${organization.organization}`} label={t("portals.hosts.base")} description={t("portals.hosts.baseHint")} className="min-w-0 flex-1 basis-56">
                <Select id={`portal-host-base-${organization.organization}`} value={base} onChange={(event) => setBase(event.target.value)}>
                  {bases.map((b) => (
                    <option key={b} value={b}>
                      .{b}
                    </option>
                  ))}
                </Select>
              </Field>
            </div>
          ) : (
            <Field id={`portal-host-whole-${organization.organization}`} label={t("portals.hosts.whole")} description={t("portals.hosts.wholeHint", { domain: verified ?? "" })} required errors={errors}>
              <Input id={`portal-host-whole-${organization.organization}`} value={whole} placeholder={`data.${verified ?? "example.org"}`} autoComplete="off" onChange={(event) => setWhole(event.target.value)} />
            </Field>
          )}
          {ready ? <p className="text-caption text-fg-muted">{t("portals.hosts.will", { host: hostOf(mode, label, base, whole) })}</p> : null}
          <Button type="submit" disabled={!ready || add.isPending}>
            {add.isPending ? t("portals.hosts.adding") : t("portals.hosts.add")}
          </Button>
        </form>
      ) : (
        <Alert tone="info" role="status">
          {t("portals.hosts.cannotAdd")}
        </Alert>
      )}
      <ConfirmDialog
        open={removing !== undefined}
        onOpenChange={(open) => {
          if (!open) setRemoving(undefined);
        }}
        title={t("portals.hosts.removeTitle", { host: removing ?? "" })}
        description={t("portals.hosts.removeDescription")}
        confirmLabel={t("portals.hosts.remove")}
        tone="danger"
        pending={remove.isPending}
        onConfirm={() => {
          if (removing) remove.mutate(removing);
        }}
      />
    </Card>
  );
}
