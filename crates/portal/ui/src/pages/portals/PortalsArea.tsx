import { useState } from "react";
import type { JSX } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, ResourceProposal } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import { useReadableList } from "../../api/permissions";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { navLinkClass } from "../../components/layout/BrandMark";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  Card,
  Checkbox,
  EmptyState,
  ExternalLink,
  Field,
  Icon,
  InlineError,
  Input,
  PageFailed,
  PageHeader,
  PageLoading,
  RadioGroup,
  Select,
} from "../../components/ui";

type PortalOrganization = components["schemas"]["PortalOrganization"];

/** The languages a portal is shown in: the Portal's own locales (ODP-08). */
export const PORTAL_LANGUAGES = ["sk", "en", "cs", "de"] as const;

/** The DCAT-AP profiles the catalogue's feeds know (dataroom profile registry). */
export const PORTAL_PROFILES = ["dcat-ap-3.0.1", "dcat-ap-sk-2.0", "dcat-ap-cz"] as const;

/** The default portal of an Organization: its own catalogue, never deleted (ODP-09). */
const DEFAULT_PATH = "default";

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

/** What `spec` of a Portal the editor reads and writes (docs/kinds/Portal.md). */
export interface PortalSpec {
  organizationRef: string;
  path: string;
  title?: Record<string, string>;
  languages?: string[];
  host?: string;
  slug?: "redirect" | "keep";
  appearance?: { instanceName?: string; colours?: { primary?: string } };
  profiles?: string[];
  sources?: unknown[];
  hidden?: string[];
}

interface PortalManifest {
  metadata: { name: string };
  spec: PortalSpec;
}

/**
 * The address a portal answers at (ODP-08): its own host, or the slug address under the
 * Organization's open data base, `default` at the base itself. `undefined` when the base is not
 * known here and the portal has no host.
 */
export function portalAddress(spec: PortalSpec, opendata: string | undefined): string | undefined {
  if (spec.host) return `https://${spec.host}/`;
  if (!opendata) return undefined;
  return spec.path === DEFAULT_PATH ? `${opendata}/` : `${opendata}/-/${spec.path}/`;
}

/** The Organizations the caller makes portals in (`GET /organization/portal-hosts`). */
export function usePortalHosts() {
  return useQuery({
    queryKey: ["portalHosts"],
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/organization/portal-hosts")),
  });
}

const BACK_LINK =
  "focus-ring flex items-center gap-2 rounded-md px-2.5 py-1.5 text-body text-fg-muted hover:bg-surface-subtle hover:text-fg";

/**
 * The sidebar of the Portals area (ODP-15, T-6875): its own navigation, apart from the project
 * pages and Administration, with a way back to Administration.
 */
export function PortalsNav({ active, onNavigate }: { active: "list" | "new" | undefined; onNavigate: () => void }): JSX.Element {
  const { t } = useTranslation();
  const hosts = usePortalHosts();
  return (
    <>
      <Link to="/organization/$tab" params={{ tab: "settings" }} onClick={onNavigate} className={BACK_LINK}>
        <Icon name="chevronLeft" className="size-4" />
        <span>{t("portals.nav.back")}</span>
      </Link>
      <div className="flex flex-col gap-1">
        <h2 id="portals-nav" className="px-2.5 text-caption font-semibold uppercase tracking-wide text-fg-subtle">
          {t("portals.nav.title")}
        </h2>
        <ul aria-labelledby="portals-nav" className="flex flex-col gap-0.5">
          <li>
            <Link to="/portals" onClick={onNavigate} aria-current={active === "list" ? "page" : undefined} className={navLinkClass(active === "list")}>
              <span className="truncate">{t("portals.nav.list")}</span>
            </Link>
          </li>
          {hosts.isSuccess ? (
            <li>
              <Link to="/portals/$name" params={{ name: "new" }} onClick={onNavigate} aria-current={active === "new" ? "page" : undefined} className={navLinkClass(active === "new")}>
                <span className="truncate">{t("portals.nav.new")}</span>
              </Link>
            </li>
          ) : null}
        </ul>
      </div>
    </>
  );
}

/** Why the area shows nothing: the caller makes portals in no Organization (403). */
function NotAdmin(): JSX.Element {
  const { t } = useTranslation();
  return (
    <Alert role="status" tone="info">
      {t("portals.notAdmin")}
    </Alert>
  );
}

function refused(error: unknown): boolean {
  return error instanceof ApiError && (error.status === 403 || error.status === 404);
}

/** The portal list (ODP-15): each portal of the caller's Organizations, its address and sources. */
export function PortalsList(): JSX.Element {
  const { t, i18n } = useTranslation();
  const hosts = usePortalHosts();
  const titleOf = (portal: PortalManifest): string =>
    portal.spec.title?.[i18n.language] ?? Object.values(portal.spec.title ?? {})[0] ?? portal.spec.path;
  const portals = useReadableList(ORG_NAMESPACE, "portals", "Portal", hosts.isSuccess);
  const header = (
    <PageHeader
      title={t("portals.list.title")}
      description={t("portals.list.description")}
      actions={
        hosts.isSuccess ? (
          <Link to="/portals/$name" params={{ name: "new" }} className={buttonClass("primary")}>
            <Icon name="plus" className="size-4" />
            {t("portals.list.new")}
          </Link>
        ) : null
      }
    />
  );
  if (hosts.isPending) return <>{header}<PageLoading label={t("app.loading")} /></>;
  if (hosts.isError) {
    return <>{header}{refused(hosts.error) ? <NotAdmin /> : <PageFailed error={hosts.error} onRetry={() => void hosts.refetch()} />}</>;
  }
  const bases = new Map(hosts.data.items.map((org) => [org.organization, org.opendata ?? undefined]));
  const items = ((portals.data?.items ?? []) as unknown as PortalManifest[]).filter((portal) => bases.has(portal.spec.organizationRef));
  if (portals.isPending) return <>{header}<PageLoading label={t("app.loading")} /></>;
  if (portals.isError) return <>{header}<PageFailed error={portals.error} onRetry={() => void portals.refetch()} /></>;
  return (
    <>
      {header}
      <ManifestGrid<PortalManifest>
        caption={t("portals.list.title")}
        storageKey="portals"
        searchLabel={t("grid.searchIn", { list: t("portals.list.title") })}
        rows={items}
        rowKey={(portal) => portal.metadata.name}
        query={{ isPending: false, isError: false, error: null, refetch: () => undefined }}
        name={{
          id: "title",
          label: t("portals.field.title"),
          text: (portal) => titleOf(portal),
          cell: (portal) => (
            <span className="inline-flex flex-wrap items-center gap-2">
              <Link to="/portals/$name" params={{ name: portal.metadata.name }} className="focus-ring font-medium underline hover:no-underline">
                {titleOf(portal)}
              </Link>
              {portal.spec.path === DEFAULT_PATH ? <Badge tone="neutral">{t("portals.list.default")}</Badge> : null}
            </span>
          ),
        }}
        columns={[
          // One Organization needs no column of it: a phone row stays short (UI judge, T-6875).
          ...(bases.size > 1
            ? [{ id: "organization", label: t("portals.field.organization"), filter: true, text: (portal: PortalManifest) => portal.spec.organizationRef, cell: (portal: PortalManifest) => portal.spec.organizationRef }]
            : []),
          {
            id: "address",
            label: t("portals.field.address"),
            text: (portal) => portalAddress(portal.spec, bases.get(portal.spec.organizationRef)) ?? "",
            cell: (portal) => {
              const address = portalAddress(portal.spec, bases.get(portal.spec.organizationRef));
              return address ? (
                <ExternalLink href={address}>{address}</ExternalLink>
              ) : (
                <span className="text-fg-muted">{t("portals.list.noBase", { path: portal.spec.path })}</span>
              );
            },
          },
          { id: "sources", label: t("portals.list.sources"), text: (portal) => String(portal.spec.sources?.length ?? 0), cell: (portal) => portal.spec.sources?.length ?? 0 },
        ]}
        empty={
          <EmptyState
            bare
            icon="globe"
            title={t("portals.list.empty")}
            description={t("portals.list.emptyHint")}
            action={
              <Link to="/portals/$name" params={{ name: "new" }} className={buttonClass("primary")}>
                {t("portals.list.new")}
              </Link>
            }
          />
        }
      />
    </>
  );
}

/** What the editor holds while a person types. */
export interface PortalDraft {
  organization: string;
  path: string;
  title: Record<string, string>;
  languages: string[];
  mode: "slug" | "domain";
  host: string;
  keepSlug: boolean;
  instanceName: string;
  primary: string;
  profiles: string[];
}

export function draftOf(spec: PortalSpec | undefined, organization: string): PortalDraft {
  return {
    organization: spec?.organizationRef ?? organization,
    path: spec?.path ?? "",
    title: { ...(spec?.title ?? {}) },
    languages: spec?.languages?.length ? [...spec.languages] : ["sk", "en"],
    mode: spec?.host ? "domain" : "slug",
    host: spec?.host ?? "",
    keepSlug: spec?.slug === "keep",
    instanceName: spec?.appearance?.instanceName ?? "",
    primary: spec?.appearance?.colours?.primary ?? "",
    profiles: spec?.profiles ? [...spec.profiles] : ["dcat-ap-3.0.1"],
  };
}

/**
 * The spec to propose: the stored one with the editor's fields over it, so the sources and the
 * hidden datasets (the sources tab's) stay as they were.
 */
export function specOf(draft: PortalDraft, stored: PortalSpec | undefined): PortalSpec {
  const spec: PortalSpec = { ...(stored ?? {}), organizationRef: draft.organization, path: draft.path };
  const title = Object.fromEntries(
    draft.languages.map((lang) => [lang, draft.title[lang]?.trim() ?? ""]).filter(([, text]) => text !== ""),
  );
  if (Object.keys(title).length > 0) spec.title = title;
  else delete spec.title;
  spec.languages = draft.languages;
  if (draft.mode === "domain" && draft.host) {
    spec.host = draft.host;
    spec.slug = draft.keepSlug ? "keep" : "redirect";
  } else {
    delete spec.host;
    delete spec.slug;
  }
  const colours = draft.primary.trim() ? { ...(stored?.appearance?.colours ?? {}), primary: draft.primary.trim() } : undefined;
  const appearance = { ...(stored?.appearance ?? {}), instanceName: draft.instanceName.trim() || undefined, colours };
  if (!appearance.instanceName) delete appearance.instanceName;
  if (!appearance.colours) delete appearance.colours;
  if (Object.keys(appearance).length > 0) spec.appearance = appearance;
  else delete spec.appearance;
  if (draft.profiles.length > 0) spec.profiles = draft.profiles;
  else delete spec.profiles;
  return spec;
}

type DraftField = "organization" | "path" | "title" | "languages" | "host" | "primary";

/** What the editor refuses before anything is sent, by field, as locale keys. */
export function faultsOf(draft: PortalDraft, creating: boolean, taken: string[]): Partial<Record<DraftField, string>> {
  const found: Partial<Record<DraftField, string>> = {};
  if (!draft.organization) found.organization = "portals.fault.organization";
  if (creating && !DNS_LABEL.test(draft.path)) found.path = "portals.fault.path";
  else if (creating && draft.path === DEFAULT_PATH) found.path = "portals.fault.pathDefault";
  else if (creating && taken.includes(draft.path)) found.path = "portals.fault.pathTaken";
  if (draft.languages.length === 0) found.languages = "portals.fault.languages";
  else if (!draft.title[draft.languages[0]]?.trim()) found.title = "portals.fault.title";
  if (draft.mode === "domain" && !draft.host) found.host = "portals.fault.host";
  if (draft.primary.trim() && !/^#[0-9a-fA-F]{6}$/.test(draft.primary.trim())) found.primary = "portals.fault.primary";
  return found;
}

/** The field a server refusal line (`spec.host: …`) belongs to; `undefined` for the form as a whole. */
export function fieldOfRefusal(line: string): DraftField | undefined {
  const member = line.split(":", 1)[0]?.trim() ?? "";
  if (member.startsWith("spec.host") || member.startsWith("spec.slug")) return "host";
  if (member.startsWith("spec.path") || member === "metadata.name") return "path";
  if (member.startsWith("spec.organizationRef")) return "organization";
  if (member.startsWith("spec.title")) return "title";
  if (member.startsWith("spec.languages")) return "languages";
  if (member.startsWith("spec.appearance")) return "primary";
  return undefined;
}

/** The text of a refusal line after its member (`spec.host: x` -> `x`). */
function reasonOfLine(line: string): string {
  const at = line.indexOf(":");
  return at > 0 && fieldOfRefusal(line) ? line.slice(at + 1).trim() : line;
}

/**
 * The portal editor (ODP-15): a new portal at `/portals/new`, an existing one at `/portals/{name}`.
 * Every write is a proposed Change; the server's judge checks the host, the path and the cap
 * again and its refusal lands on the field it names.
 */
export function PortalEditor({ name }: { name: string }): JSX.Element {
  const { t } = useTranslation();
  const creating = name === "new";
  const hosts = usePortalHosts();
  const stored = useQuery({
    queryKey: queryKeys.resource(ORG_NAMESPACE, "portals", name),
    enabled: !creating && hosts.isSuccess,
    retry: false,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}/{name}", { params: { path: { project: ORG_NAMESPACE, plural: "portals", name } } })),
  });
  const portals = useReadableList(ORG_NAMESPACE, "portals", "Portal", creating && hosts.isSuccess);
  const title = creating ? t("portals.editor.newTitle") : t("portals.editor.editTitle", { name });
  const header = <PageHeader title={title} description={t("portals.editor.description")} breadcrumb={<Link to="/portals" className="focus-ring underline hover:no-underline">{t("portals.nav.list")}</Link>} />;
  if (hosts.isPending || (!creating && stored.isPending)) return <>{header}<PageLoading label={t("app.loading")} /></>;
  if (hosts.isError) return <>{header}{refused(hosts.error) ? <NotAdmin /> : <PageFailed error={hosts.error} onRetry={() => void hosts.refetch()} />}</>;
  const manifest = stored.data as unknown as PortalManifest | undefined;
  if (!creating && (stored.isError || !manifest?.spec)) {
    return (
      <>
        {header}
        {!stored.isError || refused(stored.error) ? (
          <EmptyState
            icon="search"
            title={t("portals.editor.missing", { name })}
            description={t("portals.editor.missingHint")}
            action={
              <Link to="/portals" className={buttonClass("secondary")}>
                {t("portals.editor.backToList")}
              </Link>
            }
          />
        ) : (
          <PageFailed error={stored.error} onRetry={() => void stored.refetch()} />
        )}
      </>
    );
  }
  const taken = creating
    ? ((portals.data?.items ?? []) as unknown as PortalManifest[]).map((portal) => portal.spec.path)
    : [];
  return (
    <>
      {header}
      <PortalForm key={name} name={creating ? undefined : name} stored={manifest} organizations={hosts.data.items} taken={taken} />
    </>
  );
}

function PortalForm({
  name,
  stored,
  organizations,
  taken,
}: {
  name: string | undefined;
  stored: PortalManifest | undefined;
  organizations: PortalOrganization[];
  taken: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const creating = name === undefined;
  const [draft, setDraft] = useState<PortalDraft>(() => draftOf(stored?.spec, organizations.length === 1 ? organizations[0].organization : ""));
  const [shown, setShown] = useState<Partial<Record<DraftField, string>>>({});
  const [server, setServer] = useState<{ fields: Partial<Record<DraftField, string>>; form: string[] }>({ fields: {}, form: [] });
  const [change, setChange] = useState<Change | null>(null);
  const org = organizations.find((o) => o.organization === draft.organization);
  const isDefault = stored?.spec.path === DEFAULT_PATH;
  const mutation = useMutation({
    mutationFn: async () => {
      const metadataName = name ?? `${draft.organization}-${draft.path}`;
      const body = {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Portal",
        metadata: { name: metadataName, namespace: ORG_NAMESPACE },
        spec: specOf(draft, stored?.spec),
      } as unknown as ResourceProposal;
      return proposeChecked(ORG_NAMESPACE, "portals", body, creating);
    },
    onMutate: () => setServer({ fields: {}, form: [] }),
    onSuccess: (result) => {
      if (isChange(result)) setChange(result);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(ORG_NAMESPACE, "portals") });
    },
    onError: (error) => {
      const lines = error instanceof ApiError ? (error.problem?.errors?.length ? error.problem.errors : [error.problem?.detail ?? error.message]) : [t("app.error.generic")];
      const fields: Partial<Record<DraftField, string>> = {};
      const form: string[] = [];
      for (const line of lines) {
        const field = fieldOfRefusal(line);
        if (field && !fields[field]) fields[field] = reasonOfLine(line);
        else form.push(line);
      }
      setServer({ fields, form });
    },
  });
  const errorOf = (field: DraftField): string[] | undefined => {
    const own = shown[field] ? t(shown[field]) : server.fields[field];
    return own ? [own] : undefined;
  };
  const set = (next: Partial<PortalDraft>) => setDraft({ ...draft, ...next });
  const address = portalAddress(specOf(draft, stored?.spec), org?.opendata ?? undefined);
  if (change) {
    return (
      <div className="space-y-3">
        <ChangeNotice change={change} project={ORG_NAMESPACE} />
        <Button variant="secondary" onClick={() => void navigate({ to: "/portals" })}>
          {t("portals.editor.backToList")}
        </Button>
      </div>
    );
  }
  return (
    <form
      noValidate
      aria-labelledby="portal-form-title"
      className="max-w-3xl space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        if (mutation.isPending) return;
        const found = faultsOf(draft, creating, taken);
        setShown(found);
        const first = (Object.keys(found) as DraftField[])[0];
        if (first) {
          document.getElementById(`portal-${first}`)?.focus();
          return;
        }
        mutation.mutate();
      }}
    >
      <h2 id="portal-form-title" className="sr-only">
        {creating ? t("portals.editor.newTitle") : t("portals.editor.editTitle", { name })}
      </h2>
      {server.form.length > 0 ? (
        <Alert tone="danger" role="alert" title={t("portals.editor.refused")}>
          {server.form.join("; ")}
        </Alert>
      ) : null}
      <Card className="space-y-4">
        <Field id="portal-organization" label={t("portals.field.organization")} description={t("portals.hint.organization")} required errors={errorOf("organization")}>
          <Select
            id="portal-organization"
            value={draft.organization}
            disabled={!creating}
            title={creating ? undefined : t("portals.editor.fixed")}
            onChange={(event) => set({ organization: event.target.value, host: "" })}
          >
            <option value="">{t("portals.field.organizationPick")}</option>
            {organizations.map((o) => (
              <option key={o.organization} value={o.organization}>
                {o.organization}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="portal-path" label={t("portals.field.path")} description={t("portals.hint.path")} required errors={errorOf("path")}>
          <Input
            id="portal-path"
            value={draft.path}
            disabled={!creating}
            title={creating ? undefined : t("portals.editor.fixed")}
            autoComplete="off"
            spellCheck={false}
            placeholder="mobility"
            onChange={(event) => set({ path: event.target.value.trim().toLowerCase() })}
          />
        </Field>
        <fieldset className="space-y-2" aria-describedby="portal-languages-hint">
          <legend className="text-body font-medium text-fg">{t("portals.field.languages")}</legend>
          <p id="portal-languages-hint" className="text-caption text-fg-muted">
            {t("portals.hint.languages")}
          </p>
          <div id="portal-languages" className="flex flex-wrap gap-4">
            {PORTAL_LANGUAGES.map((lang) => (
              <Checkbox
                key={lang}
                label={t(`portals.language.${lang}`)}
                checked={draft.languages.includes(lang)}
                onChange={(event) =>
                  set({ languages: event.target.checked ? PORTAL_LANGUAGES.filter((l) => l === lang || draft.languages.includes(l)) : draft.languages.filter((l) => l !== lang) })
                }
              />
            ))}
          </div>
          {errorOf("languages") ? <InlineError>{errorOf("languages")?.[0]}</InlineError> : null}
        </fieldset>
        {draft.languages.map((lang, index) => (
          <Field
            key={lang}
            id={index === 0 ? "portal-title" : `portal-title-${lang}`}
            label={t("portals.field.titleIn", { language: t(`portals.language.${lang}`) })}
            description={t("portals.hint.title")}
            required={index === 0}
            errors={index === 0 ? errorOf("title") : undefined}
          >
            <Input
              id={index === 0 ? "portal-title" : `portal-title-${lang}`}
              value={draft.title[lang] ?? ""}
              placeholder={lang === "sk" ? "Doprava" : "Mobility"}
              onChange={(event) => set({ title: { ...draft.title, [lang]: event.target.value } })}
            />
          </Field>
        ))}
      </Card>
      <Card className="space-y-4">
        <RadioGroup<"slug" | "domain">
          name="portal-mode"
          legend={t("portals.field.address")}
          description={t("portals.hint.address")}
          value={draft.mode}
          options={[
            { value: "slug", label: t("portals.mode.slug") },
            { value: "domain", label: t("portals.mode.domain") },
          ]}
          onChange={(mode) => set({ mode })}
        />
        {draft.mode === "domain" ? (
          <>
            <Field id="portal-host" label={t("portals.field.host")} description={t("portals.hint.host")} required errors={errorOf("host")}>
              <Select id="portal-host" value={draft.host} onChange={(event) => set({ host: event.target.value })}>
                <option value="">{t("portals.field.hostPick")}</option>
                {(org?.hosts ?? []).map((host) => (
                  <option key={host} value={host}>
                    {host}
                  </option>
                ))}
              </Select>
            </Field>
            {org && org.hosts.length === 0 ? (
              <Alert tone="info" role="status">
                {t("portals.editor.noHosts", { organization: org.organization })}
              </Alert>
            ) : null}
            <p className="text-caption text-fg-muted">
              {t("portals.editor.proposeDomain")}{" "}
              <Link to="/organization/$tab" params={{ tab: "settings" }} className="focus-ring underline hover:no-underline">
                {t("portals.editor.proposeDomainLink")}
              </Link>
            </p>
            <div className="space-y-1">
              <Checkbox
                label={t("portals.field.keepSlug")}
                aria-describedby="portal-keepSlug-hint"
                checked={draft.keepSlug}
                onChange={(event) => set({ keepSlug: event.target.checked })}
              />
              <p id="portal-keepSlug-hint" className="text-caption text-fg-muted">
                {t("portals.hint.keepSlug")}
              </p>
            </div>
          </>
        ) : null}
        <p className="text-caption text-fg-muted" aria-live="polite">
          {address ? t("portals.editor.addressWill", { address }) : t("portals.editor.addressUnknown")}
        </p>
      </Card>
      <Card className="space-y-4">
        <Field id="portal-instanceName" label={t("portals.field.instanceName")} description={t("portals.hint.instanceName")}>
          <Input id="portal-instanceName" value={draft.instanceName} placeholder="Doprava Bratislava" onChange={(event) => set({ instanceName: event.target.value })} />
        </Field>
        <Field id="portal-primary" label={t("portals.field.primary")} description={t("portals.hint.primary")} errors={errorOf("primary")}>
          <Input id="portal-primary" value={draft.primary} spellCheck={false} onChange={(event) => set({ primary: event.target.value })} />
        </Field>
        <fieldset className="space-y-2" aria-describedby="portal-profiles-hint">
          <legend className="text-body font-medium text-fg">{t("portals.field.profiles")}</legend>
          <p id="portal-profiles-hint" className="text-caption text-fg-muted">
            {t("portals.hint.profiles")}
          </p>
          <div className="flex flex-wrap gap-4">
            {PORTAL_PROFILES.map((profile) => (
              <Checkbox
                key={profile}
                label={profile}
                checked={draft.profiles.includes(profile)}
                onChange={(event) =>
                  set({ profiles: event.target.checked ? [...draft.profiles, profile] : draft.profiles.filter((p) => p !== profile) })
                }
              />
            ))}
          </div>
        </fieldset>
      </Card>
      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" variant="primary" loading={mutation.isPending}>
          {creating ? t("portals.editor.create") : t("portals.editor.save")}
        </Button>
        {!creating && address ? <ExternalLink href={address}>{t("portals.editor.open")}</ExternalLink> : null}
        {!creating && name && !isDefault ? (
          <span className="sm:ml-auto">
          <DeleteResourceAction target={{ project: ORG_NAMESPACE, kind: "Portal", plural: "portals", name, label: draft.title[draft.languages[0]] || name }} label={t("portals.editor.delete")} />
          </span>
        ) : null}
        {isDefault ? <span className="text-caption text-fg-muted">{t("portals.editor.defaultKept")}</span> : null}
      </div>
    </form>
  );
}
