import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link as RouterLink } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError, queryKeys } from "../../api/client";
import { asManifests, isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { door } from "../../api/odata";
import type { RestAnswer } from "../../api/odata";
import { useReadableList } from "../../api/permissions";
import { useOrganizationManifests } from "../../api/projects";
import { useBranding } from "../../branding";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Swatch } from "../../components/forms/widgets/AppearanceWidgets";
import {
  Alert,
  Button,
  buttonClass,
  ConfirmDialog,
  Field,
  Input,
  PageFailed,
  PageLoading,
  Select,
  Tabs,
  tabPanelProps,
  Textarea,
} from "../../components/ui";
import { useIsPhone } from "../../phone";
import {
  COLOURS,
  contrastRatio,
  FONT_STACKS,
  FONTS,
  foregroundOn,
  isHexColour,
  fromForm,
  hasLook,
  MAX_INTRO_BYTES,
  MAX_LINKS,
  MAX_RADIUS,
  MIN_CONTRAST,
  lookProblems,
  toForm,
  withLook,
  wrapWords,
} from "./appearanceLook";
import type { ColourName, Look, LookForm, Link } from "./appearanceLook";
import { PagesHistory, usePreview } from "./OpendataPages";
import { previewElements } from "./pageMarkdown";

const OPEN = ["PendingApproval", "Deploying"];
type Target = { kind: "Organization" | "Portal"; name: string };

function problem(error: unknown): string {
  return error instanceof ApiError ? (error.problem?.detail ?? error.message) : String(error);
}

/** Whether a closed Change wrote a theme member of `spec.appearance`. */
function touchesLook(fields: { path: string; from?: unknown; to?: unknown }[] | null | undefined): boolean {
  return (fields ?? []).some(
    (field) =>
      /^spec\.appearance\.(colours|font|radius|hero|datasetList|navLinks|footerLinks)/.test(field.path) ||
      (field.path === "spec.appearance" &&
        /"(colours|font|radius|hero|datasetList|navLinks|footerLinks)"/.test(JSON.stringify([field.from, field.to]))),
  );
}

const appearanceOf = (manifest: Manifest | undefined) =>
  (manifest?.spec as { appearance?: Record<string, unknown> } | undefined)?.appearance;

/**
 * Administration → Open data → Appearance (T-7413, T-7410): the open data portal's colours,
 * typeface, corners, home hero, dataset list and header and footer links, for the Organization
 * or one of its portals, with a preview of the home and dataset page at 1440 and 400 px drawn
 * from the form before anything is saved. Publishing proposes the manifest as a Change an
 * approver merges; the catalogue follows the merged look within seconds, no restart. The server
 * refuses what jc-core refuses (contrast below AA, a `javascript:` link) and checks the right to
 * propose the manifest again.
 */
export function OpendataAppearance(): JSX.Element {
  const { t } = useTranslation();
  const organizations = useOrganizationManifests();
  const all = asManifests(organizations.data?.items ?? []);
  // The host's Organization; on the deployment's own address the installation's (as Settings, T-7151).
  const organization =
    all.find((item) => (item.spec as { installation?: boolean } | undefined)?.installation === true) ?? all[0];
  const orgName = organization?.metadata.name;
  // The portals the server lets this person read, of this Organization alone.
  const portalList = useReadableList(ORG_NAMESPACE, "portals", "Portal", orgName !== undefined);
  const portals = asManifests(portalList.data?.items ?? []).filter(
    (portal) => (portal.spec as { organizationRef?: string } | undefined)?.organizationRef === orgName,
  );
  const [picked, setPicked] = useState<string>("");

  if (organizations.isPending) return <PageLoading label={t("app.loading")} />;
  if (organizations.isError) {
    return <PageFailed error={organizations.error} onRetry={() => void organizations.refetch()} />;
  }
  if (!organization || !orgName) {
    return <Alert tone="info">{t("opendataAppearance.noOrganization")}</Alert>;
  }
  const portal = portals.find((item) => item.metadata.name === picked);
  const target: Target = portal ? { kind: "Portal", name: portal.metadata.name } : { kind: "Organization", name: orgName };
  const stored = portal ?? organization;
  return (
    <section aria-labelledby="opendata-appearance-title" className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 id="opendata-appearance-title" className="text-title font-semibold text-fg">
          {t("opendataAppearance.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("opendataAppearance.lead", { organization: orgName })}</p>
      </div>
      <Field
        id="appearance-target"
        label={t("opendataAppearance.target")}
        help={portals.length > 0 ? t("opendataAppearance.hint.target") : t("opendataAppearance.targetNoPortals")}
      >
        <Select id="appearance-target" value={portal ? picked : ""} onChange={(event) => setPicked(event.target.value)}>
          <option value="">{t("opendataAppearance.targetOrganization", { organization: orgName })}</option>
          {portals.map((item) => (
            <option key={item.metadata.name} value={item.metadata.name}>
              {t("opendataAppearance.targetPortal", {
                portal: item.metadata.name,
                host: (item.spec as { host?: string } | undefined)?.host ?? "",
              })}
            </option>
          ))}
        </Select>
      </Field>
      <AppearanceEditor
        key={`${target.kind}/${target.name}`}
        target={target}
        stored={stored}
        under={portal ? (appearanceOf(organization) as Look | undefined) : undefined}
      />
      <PagesHistory
        organization={target.name}
        kind={target.kind}
        touches={touchesLook}
        heading={t("opendataAppearance.history")}
        empty={t("opendataAppearance.historyEmpty")}
      />
    </section>
  );
}

/** The open Changes of the manifest in hand: a second publish would meet a 409. */
function useOpenChanges(target: Target): Change[] {
  const list = useQuery({
    queryKey: queryKeys.changes(ORG_NAMESPACE),
    queryFn: async () => (await door("portal_organization_change_list", {})) as RestAnswer<"list_changes">,
  });
  return (list.data?.items ?? []).filter((change) => {
    const params = change.summary.params as Record<string, unknown>;
    return OPEN.includes(change.status.phase) && params.kind === target.kind && params.name === target.name;
  }) as unknown as Change[];
}

function AppearanceEditor({
  target,
  stored,
  under,
}: {
  target: Target;
  stored: Manifest;
  /** For a portal: its Organization's look, which every member left empty keeps. */
  under: Look | undefined;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // The installation's look: what the catalogue draws where neither the Organization nor the portal sets a member.
  const builtIn = useBranding();
  const storedLook = appearanceOf(stored) as Look | undefined;
  const [form, setForm] = useState<LookForm>(() => toForm(storedLook));
  const [confirming, setConfirming] = useState(false);
  const open = useOpenChanges(target);
  const plural = target.kind === "Portal" ? "portals" : "organizations";
  const send = async (look: Look) => {
    const answer = await proposeChecked(ORG_NAMESPACE, plural, withLook(stored, look) as ResourceProposal, false);
    return isChange(answer) ? answer : null;
  };
  const settle = async () => {
    setConfirming(false);
    await queryClient.invalidateQueries({ queryKey: queryKeys.changes(ORG_NAMESPACE) });
  };
  const publish = useMutation({ mutationFn: () => send(fromForm(form, storedLook)), onSuccess: settle });
  const revert = useMutation({ mutationFn: () => send({}), onSuccess: settle });
  const write = publish.isSuccess ? publish : revert;
  const done = write.data ?? null;
  const error = publish.error ?? revert.error;

  // What the empty fields show: the Organization's look for a portal, then the catalogue's own.
  const shownColours = Object.fromEntries(
    COLOURS.map((name) => {
      const typed = form.colours[name].trim();
      return [name, isHexColour(typed) ? typed : under?.colours?.[name] || builtIn.colours[name]];
    }),
  ) as Record<ColourName, string>;
  const problems = lookProblems(form);
  const errorOf = (field: string) => {
    const found = problems.find((item) => item.field === field);
    return found ? [t(`opendataAppearance.problem.${found.key}`, found.values)] : undefined;
  };
  const labelOf = (field: string): string => {
    const link = /^(navLinks|footerLinks)-(\d+)-(label|url)$/.exec(field);
    if (link) {
      return `${t(`organization.field.appearance.${link[1]}`)} ${Number(link[2]) + 1}: ${t(
        `organization.field.appearance.${link[3] === "url" ? "linkUrl" : "linkLabel"}`,
      )}`;
    }
    if (field.startsWith("colour-")) return t(`organization.field.appearance.colour.${field.slice(7)}`);
    return t(field === "radius" ? "organization.field.appearance.radius" : "organization.field.appearance.heroIntro");
  };
  const anchorOf = (field: string) => (/^(navLinks|footerLinks)-/.test(field) ? field : `appearance-${field}`);
  const blocked = open.length > 0 && !done;
  const own = hasLook(appearanceOf(stored));
  // The server said this person may not propose the manifest: a second try ends the same way.
  const refusedHere = error instanceof ApiError && error.status === 403;
  const inherited = target.kind === "Portal" ? t("opendataAppearance.fromOrganization") : t("opendataAppearance.fromCatalogue");
  const setColour = (name: ColourName, value: string) =>
    setForm((now) => ({ ...now, colours: { ...now.colours, [name]: value } }));

  const look: Look = {
    ...under,
    ...fromForm(form, storedLook),
    colours: shownColours,
    font: form.font || under?.font,
    radius: form.radius.trim() === "" ? under?.radius : Number(form.radius),
  };

  return (
    <div className="flex flex-col gap-4" data-testid="opendata-appearance-editor">
      {blocked ? (
        <Alert tone="warning" id="appearance-blocked">
          <span className="flex flex-wrap items-center gap-2">
            {t("opendataAppearance.openChange")}
            {open.map((change) => (
              <RouterLink
                key={change.metadata.name}
                to="/projects/$project/approvals/$id"
                params={{ project: ORG_NAMESPACE, id: change.metadata.name }}
                className={buttonClass("secondary", "sm")}
              >
                {change.metadata.name}
              </RouterLink>
            ))}
          </span>
        </Alert>
      ) : null}
      {own || target.kind === "Portal" ? null : <Alert tone="info">{t("opendataAppearance.noneOwn")}</Alert>}
      <div className="grid gap-6 xl:grid-cols-[minmax(0,26rem)_minmax(0,1fr)]">
        {/* First on a phone, so an edit is seen without scrolling past the form; beside it and held in view on a wide screen. */}
        <div className="min-w-0 xl:sticky xl:top-4 xl:order-2 xl:self-start">
          <LookPreview
            look={look}
            name={target.name}
            fallbackFont={builtIn.fonts.body}
            refused={problems.some((item) => item.key === "contrastLow")}
          />
        </div>
        <form
          className="flex flex-col gap-4 xl:order-1"
          aria-label={t("opendataAppearance.formLabel")}
          onSubmit={(event) => {
            event.preventDefault();
            if (problems.length === 0 && !blocked) publish.mutate();
          }}
        >
          <fieldset className="flex flex-col gap-3">
            <legend className="mb-1 text-body font-semibold text-fg">{t("organization.field.appearance.colours")}</legend>
            <p className="text-caption text-fg-muted">{t("opendataAppearance.hint.colours")}</p>
            {COLOURS.map((name) => (
              <ColourField
                key={name}
                name={name}
                value={form.colours[name]}
                shown={shownColours[name]}
                background={shownColours.background}
                placeholder={inherited}
                errors={errorOf(`colour-${name}`)}
                onChange={(value) => setColour(name, value)}
              />
            ))}
          </fieldset>
          <Field id="appearance-font" label={t("organization.field.appearance.font")} help={t("opendataAppearance.hint.font")}>
            <Select
              id="appearance-font"
              value={form.font}
              onChange={(event) => setForm((now) => ({ ...now, font: event.target.value as LookForm["font"] }))}
            >
              <option value="">{inherited}</option>
              {FONTS.map((font) => (
                <option key={font} value={font}>
                  {t(`organization.field.appearance.fontChoice.${font}`)}
                </option>
              ))}
            </Select>
          </Field>
          <Field
            id="appearance-radius"
            label={t("organization.field.appearance.radius")}
            help={t("opendataAppearance.hint.radius", { max: MAX_RADIUS })}
            errors={errorOf("radius")}
          >
            <Input
              id="appearance-radius"
              type="number"
              inputMode="numeric"
              min={0}
              max={MAX_RADIUS}
              step={1}
              placeholder={inherited}
              value={form.radius}
              onChange={(event) => setForm((now) => ({ ...now, radius: event.target.value }))}
            />
          </Field>
          <Field
            id="appearance-hero-title"
            label={t("organization.field.appearance.heroTitle")}
            help={t("opendataAppearance.hint.heroTitle")}
          >
            <Input
              id="appearance-hero-title"
              maxLength={200}
              placeholder={inherited}
              value={form.heroTitle}
              onChange={(event) => setForm((now) => ({ ...now, heroTitle: event.target.value }))}
            />
          </Field>
          <Field
            id="appearance-hero-intro"
            label={t("organization.field.appearance.heroIntro")}
            help={t("opendataAppearance.hint.heroIntro", { max: MAX_INTRO_BYTES })}
            errors={errorOf("hero-intro")}
          >
            <Textarea
              id="appearance-hero-intro"
              rows={4}
              className="font-mono text-caption"
              value={form.heroIntro}
              onChange={(event) => setForm((now) => ({ ...now, heroIntro: event.target.value }))}
            />
          </Field>
          <Field
            id="appearance-dataset-list"
            label={t("organization.field.appearance.datasetList")}
            help={t("opendataAppearance.hint.datasetList")}
          >
            <Select
              id="appearance-dataset-list"
              value={form.datasetList}
              onChange={(event) =>
                setForm((now) => ({ ...now, datasetList: event.target.value as LookForm["datasetList"] }))
              }
            >
              <option value="">{inherited}</option>
              <option value="cards">{t("organization.field.appearance.datasetListChoice.cards")}</option>
              <option value="table">{t("organization.field.appearance.datasetListChoice.table")}</option>
            </Select>
          </Field>
          {(["navLinks", "footerLinks"] as const).map((member) => (
            <LinksField
              key={member}
              member={member}
              links={form[member]}
              errorOf={errorOf}
              onChange={(links) => setForm((now) => ({ ...now, [member]: links }))}
            />
          ))}
          {problems.length > 0 ? (
            <div id="appearance-fix-first" role="status" className="flex flex-col gap-1 rounded-md border border-danger p-3">
              <p className="text-body font-semibold text-danger">
                {t("opendataAppearance.fixFirst", { n: problems.length })}
              </p>
              <ul className="flex flex-wrap gap-x-4 gap-y-1">
                {[...new Set(problems.map((item) => item.field))].map((field) => (
                  <li key={field}>
                    <a href={`#${anchorOf(field)}`} className="focus-ring inline-block py-1 text-body text-danger underline">
                      {labelOf(field)}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          <Outcome change={done} error={error} />
          {done ? null : (
            <div className="flex flex-wrap gap-2">
              <Button
                type="submit"
                variant="primary"
                loading={publish.isPending}
                disabled={blocked || problems.length > 0 || refusedHere}
                aria-describedby={
                  blocked ? "appearance-blocked" : problems.length > 0 ? "appearance-fix-first" : refusedHere ? "appearance-outcome" : undefined
                }
              >
                {t("opendataAppearance.publish")}
              </Button>
              {own ? (
                <Button
                  variant="secondary"
                  disabled={blocked}
                  aria-describedby={blocked ? "appearance-blocked" : undefined}
                  onClick={() => setConfirming(true)}
                >
                  {t(target.kind === "Portal" ? "opendataAppearance.revertPortal" : "opendataAppearance.revertOrganization")}
                </Button>
              ) : null}
            </div>
          )}
        </form>
      </div>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={t("opendataAppearance.revertTitle")}
        description={
          target.kind === "Portal"
            ? t("opendataAppearance.revertBodyPortal", { name: target.name })
            : t("opendataAppearance.revertBody", { name: target.name })
        }
        confirmLabel={t(target.kind === "Portal" ? "opendataAppearance.revertPortal" : "opendataAppearance.revertOrganization")}
        pending={revert.isPending}
        onConfirm={() => revert.mutate()}
      />
    </div>
  );
}

/** A colour as `#rrggbb` text with its swatch, and its contrast with the background, live. */
function ColourField({
  name,
  value,
  shown,
  background,
  placeholder,
  errors,
  onChange,
}: {
  name: ColourName;
  value: string;
  shown: string;
  background: string;
  placeholder: string;
  errors: string[] | undefined;
  onChange: (value: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const id = `appearance-colour-${name}`;
  const ratio = name === "text" || name === "primary" ? contrastRatio(shown, background) : null;
  const ok = ratio !== null && ratio >= MIN_CONTRAST;
  const aside = (
    <span className="flex items-center gap-2">
      {ratio !== null ? (
        <span
          data-testid={`contrast-${name}`}
          className={ok ? "text-caption text-success" : "text-caption font-semibold text-danger"}
        >
          {t(ok ? "opendataAppearance.contrastOk" : "opendataAppearance.contrastLow", {
            ratio: ratio.toFixed(2),
            min: MIN_CONTRAST,
          })}
        </span>
      ) : null}
      <Swatch colour={value.trim() || shown} className="h-control-xs w-control-xs" />
    </span>
  );
  return (
    <Field
      id={id}
      label={t(`organization.field.appearance.colour.${name}`)}
      help={t(`opendataAppearance.hint.${name}`)}
      errors={errors}
      aside={aside}
    >
      <Input
        id={id}
        className="font-mono"
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        maxLength={7}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </Field>
  );
}

function LinksField({
  member,
  links,
  errorOf,
  onChange,
}: {
  member: "navLinks" | "footerLinks";
  links: Link[];
  errorOf: (field: string) => string[] | undefined;
  onChange: (links: Link[]) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const set = (at: number, change: Partial<Link>) =>
    onChange(links.map((link, index) => (index === at ? { ...link, ...change } : link)));
  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="mb-1 text-body font-semibold text-fg">{t(`organization.field.appearance.${member}`)}</legend>
      <p className="text-caption text-fg-muted">{t(`opendataAppearance.hint.${member}`, { max: MAX_LINKS })}</p>
      {links.length === 0 ? <p className="text-caption text-fg-muted">{t("opendataAppearance.linksEmpty")}</p> : null}
      {links.map((link, at) => (
        <div key={at} className="grid gap-2 rounded-md border border-border p-3">
          <Field id={`${member}-${at}-label`} label={t("organization.field.appearance.linkLabel")} errors={errorOf(`${member}-${at}-label`)}>
            <Input
              id={`${member}-${at}-label`}
              maxLength={80}
              placeholder={t("opendataAppearance.hint.linkLabel")}
              value={link.label}
              onChange={(event) => set(at, { label: event.target.value })}
            />
          </Field>
          <Field id={`${member}-${at}-url`} label={t("organization.field.appearance.linkUrl")} errors={errorOf(`${member}-${at}-url`)}>
            <Input
              id={`${member}-${at}-url`}
              inputMode="url"
              spellCheck={false}
              placeholder="https://www.example.sk/ · /about"
              value={link.url}
              onChange={(event) => set(at, { url: event.target.value })}
            />
          </Field>
          <Button
            size="sm"
            variant="ghost"
            className="justify-self-end"
            aria-label={t("opendataAppearance.removeLink", { label: link.label || at + 1 })}
            onClick={() => onChange(links.filter((_, index) => index !== at))}
          >
            {t("opendataAppearance.remove")}
          </Button>
        </div>
      ))}
      {links.length < MAX_LINKS ? (
        <Button size="sm" variant="secondary" className="self-start" onClick={() => onChange([...links, { label: "", url: "" }])}>
          {t("opendataAppearance.addLink")}
        </Button>
      ) : null}
    </fieldset>
  );
}

/** Where a publish ended: its Change, or the server's refusal in words to act on. */
function Outcome({ change, error }: { change: Change | null; error: unknown }): JSX.Element | null {
  const { t } = useTranslation();
  if (change) {
    return (
      <div className="flex flex-col gap-2">
        <ChangeNotice change={change} project={ORG_NAMESPACE} />
        <p className="text-caption text-fg-muted">{t("opendataAppearance.waits")}</p>
      </div>
    );
  }
  if (!error) return null;
  const status = error instanceof ApiError ? error.status : 0;
  return (
    <Alert tone={status === 409 ? "warning" : "danger"} id="appearance-outcome">
      {status === 403 || status === 409 || status === 422
        ? t(`opendataAppearance.refused.${status}`, { detail: problem(error) })
        : problem(error)}
    </Alert>
  );
}

/**
 * The open data portal's home and dataset page drawn from the form, at 1440 or 400 px, with the catalogue's own colours, typeface and corner radius: what the published look
 * shows, before anything is saved. Drawn as SVG, so every colour is a presentation attribute and
 * nothing of the person's input becomes CSS. The rows are labelled examples; the hero
 * introduction goes through the catalogue's Markdown renderer and is rebuilt as elements.
 */
function LookPreview({
  look,
  name,
  fallbackFont,
  refused,
}: {
  look: Look;
  name: string;
  fallbackFont: string;
  /** A colour pair below AA: the preview draws it anyway, so the person sees why it is refused. */
  refused: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const phone = useIsPhone();
  const [page, setPage] = useState<"home" | "dataset">("home");
  // A phone shows the phone's page until the person picks the other width.
  const [picked, setWidth] = useState<"1440" | "400" | null>(null);
  const width = picked ?? (phone ? "400" : "1440");
  const intro = usePreview(look.hero?.intro ?? "");
  const introHtml = look.hero?.intro && intro.data ? previewElements(intro.data.html) : null;
  const pages = [
    { value: "home" as const, label: t("opendataAppearance.previewHome") },
    { value: "dataset" as const, label: t("opendataAppearance.previewDataset") },
  ];
  const widths = [
    { value: "1440" as const, label: t("opendataAppearance.previewWidth", { width: 1440 }) },
    { value: "400" as const, label: t("opendataAppearance.previewWidth", { width: 400 }) },
  ];
  return (
    <section aria-labelledby="appearance-preview" className="flex min-w-0 flex-col gap-3">
      <h3 id="appearance-preview" className="text-body font-semibold text-fg">
        {t("opendataAppearance.preview")}
      </h3>
      <p className="text-caption text-fg-muted">{t("opendataAppearance.previewLead")}</p>
      {refused ? <Alert tone="warning">{t("opendataAppearance.previewRefused")}</Alert> : null}
      <div className="flex flex-wrap items-end justify-between gap-2">
        <Tabs id="appearance-preview-page" label={t("opendataAppearance.previewPage")} tabs={pages} value={page} onChange={setPage} />
        <Tabs
          id="appearance-preview-width"
          label={t("opendataAppearance.previewWidths")}
          tabs={widths}
          value={width}
          onChange={setWidth}
          variant="pill"
        />
      </div>
      <div {...tabPanelProps("appearance-preview-page", page)}>
        <div
          {...tabPanelProps("appearance-preview-width", width)}
          className={width === "400" ? "mx-auto w-full max-w-sm overflow-hidden rounded-md border border-border" : "overflow-hidden rounded-md border border-border"}
        >
          <PortalPage look={look} name={name} page={page} width={width === "400" ? 400 : 1440} intro={introHtml} fallbackFont={fallbackFont} />
        </div>
      </div>
    </section>
  );
}

const EXAMPLES = [1, 2, 3] as const;

function PortalPage({
  look,
  name,
  page,
  width,
  intro,
  fallbackFont,
}: {
  look: Look;
  name: string;
  page: "home" | "dataset";
  width: 1440 | 400;
  intro: ReactNode;
  /** The installation's body font, where no typeface is picked. */
  fallbackFont: string;
}): JSX.Element {
  const { t } = useTranslation();
  const colours = look.colours as Record<ColourName, string>;
  const radius = look.radius ?? 8;
  const narrow = width === 400;
  const pad = narrow ? 16 : 48;
  const inner = width - 2 * pad;
  // The catalogue's heading sizes, its title broken into lines as a page of that width would.
  const titleSize = narrow ? 28 : 44;
  const wrap = (text: string, size: number) => wrapWords(text, Math.floor(inner / (size * 0.62)), 3);
  const title = wrap(look.hero?.title || t("opendataAppearance.heroDefault", { name }), titleSize);
  const introHeight = intro ? (narrow ? 72 : 56) : 0;
  const hero = 24 + title.length * titleSize * 1.2 + introHeight + 24;
  const examples = EXAMPLES.map((n) => t("opendataAppearance.exampleDataset", { n }));
  const nav = [t("opendataAppearance.navDatasets"), ...(look.navLinks ?? []).map((link) => link.label)];
  const footer = [...(look.footerLinks ?? []).map((link) => link.label), name];
  // The catalogue's hairline: the text colour faint over the background.
  const line = { fill: colours.text, fillOpacity: 0.12 };
  const outline = { fill: "none", stroke: colours.text, strokeOpacity: 0.15 };
  const top = narrow ? 100 : 72;
  let body: JSX.Element;
  let height: number;
  if (page === "home") {
    const listTop = top + hero + 64;
    const table = look.datasetList === "table" && !narrow;
    const cardWidth = narrow ? inner : (inner - 32) / 3;
    const cardHeight = look.datasetList === "table" ? 96 : 120;
    height = listTop + (narrow ? 3 * (cardHeight + 12) : table ? 4 * 48 : cardHeight) + 120;
    body = (
      <>
        <rect x={pad} y={top + hero} width={inner} height={44} rx={radius} {...outline} />
        <text x={pad + 14} y={top + hero + 28} fontSize={16} fillOpacity={0.7}>
          {t("opendataAppearance.searchExample")}
        </text>
        {table ? (
          <g>
            <rect x={pad} y={listTop} width={inner} height={4 * 48} rx={radius} {...outline} />
            {[t("opendataAppearance.colTitle"), ...examples].map((title, at) => (
              <g key={title}>
                {at > 0 ? <rect x={pad} y={listTop + at * 48} width={inner} height={1} {...line} /> : null}
                <text x={pad + 16} y={listTop + at * 48 + 30} fontSize={16} fontWeight={at === 0 ? 700 : 400} fill={at === 0 ? colours.text : colours.primary}>
                  {title}
                </text>
                <text x={pad + inner / 2} y={listTop + at * 48 + 30} fontSize={16} fontWeight={at === 0 ? 700 : 400}>
                  {at === 0 ? t("opendataAppearance.colPublisher") : name}
                </text>
              </g>
            ))}
          </g>
        ) : (
          examples.map((title, at) => {
            const x = narrow ? pad : pad + at * (cardWidth + 16);
            const y = narrow ? listTop + at * (cardHeight + 12) : listTop;
            return (
              <g key={title}>
                <rect x={x} y={y} width={cardWidth} height={cardHeight} rx={radius} {...outline} />
                <text x={x + 16} y={y + 34} fontSize={18} fontWeight={700} fill={colours.primary}>
                  {title}
                </text>
                <text x={x + 16} y={y + 64} fontSize={15}>
                  {look.datasetList === "table" ? `${t("opendataAppearance.colPublisher")}: ${name}` : name}
                </text>
              </g>
            );
          })
        )}
      </>
    );
  } else {
    const badge = foregroundOn(colours.accent);
    const button = foregroundOn(colours.primary);
    height = top + 360;
    body = (
      <>
        <rect x={pad} y={top + 24} width={56} height={26} rx={Math.min(radius, 13)} fill={colours.accent} />
        <text x={pad + 28} y={top + 42} fontSize={14} fontWeight={700} textAnchor="middle" fill={badge}>
          CSV
        </text>
        <Lines lines={wrap(examples[0], titleSize)} x={pad} y={top + 72 + titleSize} size={titleSize} />
        <Lines
          lines={wrap(t("opendataAppearance.exampleDescription", { name }), 16)}
          x={pad}
          y={top + 72 + titleSize * 2.4}
          size={16}
          weight={400}
        />
        <rect x={pad} y={top + 220} width={inner} height={72} rx={radius} {...outline} />
        <text x={pad + 16} y={top + 262} fontSize={16}>
          {t("opendataAppearance.exampleDistribution")}
        </text>
        <rect x={pad + inner - 136} y={top + 238} width={120} height={36} rx={radius} fill={colours.primary} />
        <text x={pad + inner - 76} y={top + 261} fontSize={15} fontWeight={600} textAnchor="middle" fill={button}>
          {t("opendataAppearance.download")}
        </text>
      </>
    );
  }
  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="block h-auto w-full"
      role="img"
      aria-label={t("opendataAppearance.previewAlt", {
        page: t(page === "home" ? "opendataAppearance.previewHome" : "opendataAppearance.previewDataset"),
        width,
      })}
      data-testid={`preview-${width}`}
      fontFamily={look.font ? FONT_STACKS[look.font] : fallbackFont}
      fill={colours.text}
      color={colours.text}
    >
      <rect width={width} height={height} fill={colours.background} />
      <text x={pad} y={40} fontSize={22} fontWeight={700} fill={colours.primary}>
        {name}
      </text>
      <text x={narrow ? pad : width - pad} y={narrow ? 80 : 40} fontSize={narrow ? 15 : 16} textAnchor={narrow ? "start" : "end"}>
        {nav.map((label, at) => (
          <tspan key={at} dx={at === 0 ? 0 : 24} textDecoration={at > 0 ? "underline" : undefined}>
            {label}
          </tspan>
        ))}
      </text>
      <rect y={top - 8} width={width} height={1} {...line} />
      {page === "home" ? (
        <g>
          <Lines lines={title} x={pad} y={top + 24 + titleSize} size={titleSize} />
          {intro ? (
            <foreignObject x={pad} y={top + 24 + title.length * titleSize * 1.2 + 8} width={inner} height={introHeight}>
              <div
                className={`flex flex-col gap-1 overflow-hidden [&_a]:underline [&_ul]:list-disc [&_ul]:pl-5 ${narrow ? "text-body" : "text-title"}`}
              >
                {intro}
              </div>
            </foreignObject>
          ) : null}
        </g>
      ) : null}
      {body}
      <rect y={height - 64} width={width} height={1} {...line} />
      <text x={pad} y={height - 26} fontSize={15}>
        {footer.map((label, at) => (
          <tspan key={at} dx={at === 0 ? 0 : 28} textDecoration={at < footer.length - 1 ? "underline" : undefined}>
            {label}
          </tspan>
        ))}
      </text>
    </svg>
  );
}

/** Lines of SVG text, one under the other, in the catalogue's heading weight unless told. */
function Lines({ lines, x, y, size, weight = 700 }: { lines: string[]; x: number; y: number; size: number; weight?: number }): JSX.Element {
  return (
    <text x={x} y={y} fontSize={size} fontWeight={weight}>
      {lines.map((line, at) => (
        <tspan key={at} x={x} dy={at === 0 ? 0 : size * 1.2}>
          {line}
        </tspan>
      ))}
    </text>
  );
}
