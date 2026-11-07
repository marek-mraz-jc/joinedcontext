import { Card } from "../../components/ui";
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { api, ApiError, unwrap, reasonOf } from "../../api/client";
import { proposeChecked } from "../../api/proposal";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert } from "../../components/ui/Alert";
import { Button } from "../../components/ui/Button";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { Field } from "../../components/ui/Field";
import { EmptyState } from "../../components/ui/EmptyState";
import { ListFailed } from "../../components/forms/widgets/ListFailed";
import { Textarea } from "../../components/ui/Input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui/Table";

type ProfileAccess = components["schemas"]["ProfileAccess"];

/**
 * What each agent profile lets a run you start do (UI-56, AG-70): per operation the profile's
 * half, your own permission and the reason one refuses. An edit of the access block is a
 * Change on the profile, reviewed like any manifest (CC-03).
 */
export function AgentAccess({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const access = useQuery({
    queryKey: ["projects", project, "assistant", "access"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/assistant/access", {
          params: { path: { project } },
        }),
      ),
  });

  return (
    <Card as="section" aria-labelledby="agent-access-heading" className="space-y-4">
      <div className="space-y-1">
        <h2 id="agent-access-heading" className="text-lg font-semibold text-fg">
          {t("assistantPage.access.title")}
        </h2>
        <p className="text-caption text-fg-muted">{t("assistantPage.access.lead")}</p>
      </div>
      {/* The order matters: in flight, then failed, then empty. `isPending` was not read at all,
          so while the request was on its way both branches fell through and the panel rendered
          this heading over blank space — on a slow link a person concluded the agent held no
          profiles and no access, which is the opposite of what this panel exists to say. */}
      {access.isPending ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("app.loading")}
        </p>
      ) : access.isError ? (
        // `app.error.generic` threw the server's own sentence away, so a 403 (you may not read
        // this project's access) and a 500 read identically and neither could be acted on.
        <ListFailed
          what={t("assistantPage.access.title")}
          reason={reasonOf(access.error, t("app.error.generic"))}
          onRetry={() => void access.refetch()}
        />
      ) : access.data?.items.length === 0 ? (
        <EmptyState
          bare
          icon="access"
          title={t("assistantPage.access.none")}
          description={t("assistantPage.access.noneHint")}
        />
      ) : (
        access.data?.items.map((profile) => (
          <ProfileCard key={profile.name} project={project} profile={profile} />
        ))
      )}
    </Card>
  );
}

function ProfileCard({ project, profile }: { project: string; profile: ProfileAccess }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const propose = useMutation({
    mutationFn: async (block: unknown) => {
      const path = { project: ORG_NAMESPACE, plural: "agentprofiles", name: profile.name };
      // `as unknown as { spec: … }` was a double cast: it silenced the compiler rather than
      // describing the value, so a shape change in the API was met when someone proposed an
      // access change to a live profile instead of at build. `Manifest` is that shape — the
      // envelope with its `spec` as a record — and the envelope's own `spec` is the only part
      // that differs, so one assertion of the declared type replaces two of nothing.
      const current = (await unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", { params: { path } }),
      )) as Manifest;
      const manifest: Record<string, unknown> = { ...current, spec: { ...current.spec } };
      delete manifest.status;
      const spec = manifest.spec as Record<string, unknown>;
      if (block === null || block === undefined) {
        delete spec.access;
      } else {
        spec.access = block;
      }
      return proposeChecked(
        path.project,
        path.plural,
        manifest as ResourceProposal,
        false,
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setText(null);
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "assistant", "access"] });
    },
    onError: (err) => {
      setError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : String(err));
    },
  });

  function submit() {
    setError(null);
    let block: unknown;
    try {
      block = parseYaml(text ?? "");
    } catch (err) {
      setError(t("assistantPage.access.yamlError", { message: err instanceof Error ? err.message : String(err) }));
      return;
    }
    propose.mutate(block);
  }

  const available = profile.operations.filter((op) => op.profile && op.person).length;
  const heading = `access-${profile.name}`;

  return (
    <article aria-labelledby={heading} className="space-y-3 rounded-md border border-border p-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h3 id={heading} className="font-semibold text-fg">
          {/* The seeded profiles by their name in the page's language; the manifest's title
              is English only, and the id is a quiet second line (T-4160 #31). */}
          {t(`assistantPage.access.profileTitles.${profile.name}`, { defaultValue: profile.title ?? profile.name })}
          <span className="block text-caption font-normal text-fg-muted">
            {t("assistantPage.access.ids", { name: profile.name, role: profile.role })}
          </span>
        </h3>
        <span className="text-caption text-fg-muted">
          {t("assistantPage.access.available", { count: available, total: profile.operations.length })}
        </span>
      </div>
      <p className="text-caption text-fg-muted">
        {profile.access ? t("assistantPage.access.declared") : t("assistantPage.access.readOnlyDefault")}
      </p>
      <p className="text-caption">
        <span className="font-medium">{t("assistantPage.access.egress")}: </span>
        <span title={profile.egressHosts.join(", ") || undefined}>{egressWords(profile.egressHosts, t)}</span>
      </p>

      {/* Folded (T-2760): every profile's every operation, open at once, made the page ten
          thousand pixels tall; the count above says what a person needs first. */}
      <details className="group">
        <summary className="focus-ring cursor-pointer rounded-sm text-caption font-medium text-primary-soft-fg">
          {t("assistantPage.access.showOperations", { count: profile.operations.length })}
        </summary>
      <Table caption={t("assistantPage.access.caption", { name: profile.name })}>
        <TableHead>
          <TableHeaderCell>{t("assistantPage.access.operation")}</TableHeaderCell>
          <TableHeaderCell>{t("assistantPage.access.profile")}</TableHeaderCell>
          <TableHeaderCell>{t("assistantPage.access.you")}</TableHeaderCell>
          <TableHeaderCell>{t("assistantPage.access.reason")}</TableHeaderCell>
        </TableHead>
        <TableBody>
          {profile.operations.map((op) => (
            <TableRow key={op.name}>
              <TableCell className="font-mono text-caption">{op.name}</TableCell>
              <TableCell>{op.profile ? t("assistantPage.access.yes") : t("assistantPage.access.no")}</TableCell>
              <TableCell>{op.person ? t("assistantPage.access.yes") : t("assistantPage.access.no")}</TableCell>
              <TableCell className="text-caption text-fg-muted">{op.reason ?? ""}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      </details>

      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}

      {text === null ? (
        // The block is proposed as the org's `AgentProfile`, so that is the permission the editor
        // needs — not one in the project being read (T-1584, UI-44).
        <PermissionGuard project={ORG_NAMESPACE} kind="AgentProfile" verb="propose">
          <Button
            size="sm"
            onClick={() => {
              setChange(null);
              setText(profile.access ? stringifyYaml(profile.access) : "");
            }}
          >
            {t("assistantPage.access.edit")}
          </Button>
        </PermissionGuard>
      ) : (
        <form
          className="space-y-2"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <Field
            id={`access-yaml-${profile.name}`}
            label={t("assistantPage.access.block")}
            help={t("assistantPage.access.blockHint")}
          >
            <Textarea
              id={`access-yaml-${profile.name}`}
              rows={8}
              className="font-mono"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
          </Field>
          {/* The refusal of a proposal is the one message in this panel a person has to act on,
              and it was a bare paragraph: no icon, no danger surface, no tone word, so it read
              as ordinary text at the moment it mattered most. */}
          {error ? <Alert tone="danger">{error}</Alert> : null}
          <div className="flex gap-2">
            <PermissionGuard project={ORG_NAMESPACE} kind="AgentProfile" verb="propose">
              <Button type="submit" variant="primary" size="sm" loading={propose.isPending}>
                {t("assistantPage.access.propose")}
              </Button>
            </PermissionGuard>
            <Button size="sm" onClick={() => setText(null)}>
              {t("assistantPage.access.cancel")}
            </Button>
          </div>
        </form>
      )}
    </article>
  );
}

/** The package registries an app build installs from: one phrase, not a row of host names. */
const PACKAGE_HOSTS = new Set([
  "registry.npmjs.org",
  "static.crates.io",
  "index.crates.io",
  "crates.io",
  "pypi.org",
  "files.pythonhosted.org",
]);

/**
 * The hosts a profile may reach, in words where they have a purpose the page can name (T-4264,
 * T-4160 #31): "balíčky pre stavbu aplikácií, api.example.org". Any other host stays as written.
 */
export function egressWords(hosts: string[], t: (key: string) => string): string {
  if (hosts.length === 0) return t("assistantPage.access.noEgress");
  const others = hosts.filter((host) => !PACKAGE_HOSTS.has(host));
  const words = others.length < hosts.length ? [t("assistantPage.access.packageHosts")] : [];
  return [...words, ...others].join(", ");
}
