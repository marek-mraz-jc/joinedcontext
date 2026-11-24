import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { useOrganizationManifests } from "../../api/projects";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Swatch } from "../../components/forms/widgets/AppearanceWidgets";
import { Alert, Badge, Button, Card, Field, Input, Select } from "../../components/ui";
import type { PortalSpec } from "./PortalsArea";
import { linesOf, usePortalChange } from "./PortalSources";
import { inherited, sourceOf, STYLE_GROUPS, styleFaults, valueAt, withValue } from "./portalAppearance";
import type { Appearance, StyleField } from "./portalAppearance";

const idOf = (path: string) => `appearance-${path.replace(/\./g, "-")}`;

/**
 * The Appearance tab of a portal (T-7744): every style member of `spec.appearance` with where it
 * shows from (this portal, its Organization or the installation's default) and a reset to that
 * level. Colours keep WCAG AA contrast in pairs; publishing proposes a Change for an approver.
 */
export function PortalAppearanceTab({ name, spec }: { name: string; spec: PortalSpec }): JSX.Element {
  const { t } = useTranslation();
  const organizations = useOrganizationManifests();
  const organization = (organizations.data?.items.find((o) => o.metadata.name === spec.organizationRef)?.spec as { appearance?: Appearance } | undefined)?.appearance;
  const stored = (spec.appearance as Appearance | undefined) ?? {};
  const [look, setLook] = useState<Appearance>(stored);
  const [tried, setTried] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const [refusal, setRefusal] = useState<string[]>([]);
  const propose = usePortalChange(name);
  const faults = styleFaults(look);
  const dirty = JSON.stringify(look) !== JSON.stringify(stored);

  const set = (path: string, value: string | number | undefined) => {
    let next = withValue(look, path, value);
    // `font` set both fonts; a font chosen alone replaces it with the two members it stood for.
    if ((path === "headingFont" || path === "bodyFont") && next.font !== undefined) {
      const legacy = next.font as string;
      next = withValue(withValue(withValue(next, "font", undefined), "headingFont", valueAt(next, "headingFont") ?? legacy), "bodyFont", valueAt(next, "bodyFont") ?? legacy);
      if (value === undefined) next = withValue(next, path, undefined);
    }
    setLook(next);
  };

  const publish = () => {
    setTried(true);
    setRefusal([]);
    if (Object.keys(faults).length > 0) return;
    const proposed = { ...spec } as PortalSpec & { appearance?: unknown };
    if (Object.keys(look).length > 0) proposed.appearance = look as PortalSpec["appearance"];
    else delete proposed.appearance;
    propose.mutate(proposed, {
      onSuccess: (result) => {
        if (isChange(result)) setChange(result);
        setTried(false);
      },
      onError: (error) => setRefusal(linesOf(error, t("app.error.generic"))),
    });
  };

  const refusedAt = (path: string) => refusal.find((line) => line.startsWith(`spec.appearance.${path}:`) || line.startsWith(`spec.appearance.${path} `));

  const control = (field: StyleField, id: string, value: string | number | undefined, shown: string | number | undefined, invalid: boolean) => {
    if (field.kind === "choice") {
      return (
        <Select id={id} value={value === undefined ? "" : String(value)} aria-invalid={invalid || undefined} onChange={(e) => set(field.path, e.currentTarget.value || undefined)}>
          <option value="">{t("portals.appearance.inheritOption", { value: shown === undefined ? t("portals.appearance.default") : t(`portals.appearance.choice.${String(shown)}`) })}</option>
          {(field.choices ?? []).map((choice) => (
            <option key={choice} value={choice}>
              {t(`portals.appearance.choice.${choice}`)}
            </option>
          ))}
        </Select>
      );
    }
    if (field.kind === "number") {
      return (
        <Input
          id={id}
          type="number"
          inputMode="numeric"
          min={field.min}
          max={field.max}
          value={value === undefined ? "" : String(value)}
          placeholder={shown === undefined ? "" : String(shown)}
          aria-invalid={invalid || undefined}
          onChange={(e) => {
            const raw = e.currentTarget.value;
            set(field.path, raw === "" ? undefined : Number(raw));
          }}
        />
      );
    }
    if (field.kind === "colour") {
      return (
        <div className="flex items-center gap-2">
          <Swatch colour={typeof value === "string" ? value : typeof shown === "string" ? shown : undefined} className="h-control w-control" />
          <Input id={id} aria-label={t(`portals.appearance.field.${field.path}`)} value={value === undefined ? "" : String(value)} placeholder={shown === undefined ? "" : String(shown)} maxLength={7} aria-invalid={invalid || undefined} onChange={(e) => set(field.path, e.currentTarget.value.trim() || undefined)} />
        </div>
      );
    }
    return (
      <Input
        id={id}
        aria-label={t(`portals.appearance.field.${field.path}`)}
        type={field.kind === "email" ? "email" : "text"}
        value={value === undefined ? "" : String(value)}
        placeholder={shown === undefined ? "" : String(shown)}
        maxLength={field.kind === "email" ? 254 : field.kind === "licence" ? 64 : 120}
        aria-invalid={invalid || undefined}
        onChange={(e) => set(field.path, e.currentTarget.value === "" ? undefined : e.currentTarget.value)}
      />
    );
  };

  return (
    <div className="space-y-4">
      <div>
        <h2 className="text-title font-semibold">{t("portals.appearance.heading")}</h2>
        <p className="text-body text-fg-muted">{t("portals.appearance.lead")}</p>
      </div>
      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}
      {refusal.length > 0 ? (
        <Alert tone="danger" role="alert" title={t("portals.appearance.refused")}>
          <p>{t("portals.appearance.refusedHint")}</p>
          <ul className="list-disc ps-5">
            {refusal.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {tried && Object.keys(faults).length > 0 ? (
        <Alert tone="danger" role="alert" title={t("portals.appearance.fix")}>
          {t("portals.appearance.fixHint", { count: Object.keys(faults).length })}
        </Alert>
      ) : null}
      {STYLE_GROUPS.map((group) => (
        <Card key={group.key} padding="md">
          <section aria-labelledby={`appearance-${group.key}`} className="space-y-3">
            <h3 id={`appearance-${group.key}`} className="text-body font-semibold">
              {t(`portals.appearance.group.${group.key}`)}
            </h3>
            <div className="grid gap-x-4 gap-y-3 md:grid-cols-2">
              {group.fields.map((field) => {
                const id = idOf(field.path);
                const own = inherited(look, field.path);
                const from = sourceOf(look, organization, field.path);
                const shown = own ?? inherited(organization, field.path);
                const fault = tried || own !== undefined ? faults[field.path] : undefined;
                const refused = refusedAt(field.path);
                const errors = [fault ? t(fault) : undefined, refused].filter((e): e is string => e !== undefined);
                return (
                  <Field
                    key={field.path}
                    id={id}
                    label={t(`portals.appearance.field.${field.path}`)}
                    help={t(`portals.appearance.hint.${field.path.replace(/\./g, "_")}`)}
                    errors={errors.length > 0 ? errors : undefined}
                    aside={
                      <span className="flex flex-wrap items-center gap-1">
                        <Badge tone={from === "portal" ? "primary" : "neutral"}>{t(`portals.appearance.from.${from}`)}</Badge>
                        {own !== undefined ? (
                          <Button size="sm" variant="ghost" aria-label={t("portals.appearance.resetField", { field: t(`portals.appearance.field.${field.path}`) })} onClick={() => set(field.path, undefined)}>
                            {t("portals.appearance.reset")}
                          </Button>
                        ) : null}
                      </span>
                    }
                  >
                    {control(field, id, own, shown, errors.length > 0)}
                  </Field>
                );
              })}
            </div>
          </section>
        </Card>
      ))}
      <Card padding="md" className={dirty ? "sticky bottom-2 z-10 flex flex-wrap items-center gap-3 shadow-md" : "flex flex-wrap items-center gap-3"}>
        <p className="min-w-0 flex-1 basis-60 text-body text-fg-muted">{dirty ? t("portals.appearance.unpublished") : t("portals.appearance.published")}</p>
        <div className="flex flex-wrap gap-2">
          <Button variant="secondary" disabled={!dirty || propose.isPending} disabledReason={t("portals.appearance.why")} onClick={() => setLook(stored)}>
            {t("portals.appearance.discard")}
          </Button>
          <Button variant="primary" disabled={!dirty} disabledReason={t("portals.appearance.why")} loading={propose.isPending} onClick={publish}>
            {t("portals.appearance.publish")}
          </Button>
        </div>
      </Card>
    </div>
  );
}
