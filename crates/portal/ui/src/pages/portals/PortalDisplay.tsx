import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { useOrganizationManifests } from "../../api/projects";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Badge, Button, Card, Checkbox, Field, InlineError, Input } from "../../components/ui";
import type { PortalSpec } from "./PortalsArea";
import { linesOf, usePortalChange } from "./PortalSources";
import { completed, displayFaults, displayOf, EXTRA_KEY, fieldRowsOf, moved, resolved } from "./portalDisplay";
import type { Display, DisplayLevel, DisplayList, DisplayRow } from "./portalDisplay";

type Lists = Partial<Record<DisplayList, DisplayRow[]>>;

const LISTS: DisplayList[] = ["sections", "about", "fields"];

function storedDisplay(spec: { appearance?: unknown } | undefined): Display | undefined {
  return (spec?.appearance as { display?: Display } | undefined)?.display;
}

/** The lists the portal sets itself, as editor rows; a list it inherits is absent. */
export function ownLists(own: Display | undefined, organization: Display | undefined, fieldRows: string[] = []): Lists {
  const lists: Lists = {};
  for (const list of LISTS) {
    const shown = resolved(own, organization, list, fieldRows);
    if (shown.from === "portal") lists[list] = shown.rows;
  }
  return lists;
}

/**
 * The Display tab of a portal (T-7743): the dataset page's sections, the about table's rows and
 * the dataset list's fields, each list inherited from the Organization or the built-in page until
 * the portal sets its own, and reset to inherited again. Publishing proposes a Change.
 */
export function PortalDisplayTab({ name, spec }: { name: string; spec: PortalSpec }): JSX.Element {
  const { t, i18n } = useTranslation();
  const organizations = useOrganizationManifests();
  const orgSpec = organizations.data?.items.find((o) => o.metadata.name === spec.organizationRef)?.spec as
    | { appearance?: unknown; datasetFields?: { name: string; label?: Record<string, string> }[] }
    | undefined;
  const organization = storedDisplay(orgSpec);
  // T-7746: the Organization's own dataset fields are about rows too, labelled as it names them.
  const fieldRows = fieldRowsOf(orgSpec?.datasetFields);
  const fieldLabels = new Map(
    (orgSpec?.datasetFields ?? []).map((f) => [`extras.x_${f.name}`, f.label?.[i18n.resolvedLanguage ?? "en"] ?? Object.values(f.label ?? {})[0] ?? f.name]),
  );
  const own = storedDisplay(spec);
  const languages = spec.languages?.length ? spec.languages : ["sk"];
  const [lists, setLists] = useState<Lists>(() => ownLists(own, organization, fieldRows));
  const [extra, setExtra] = useState("");
  const [tried, setTried] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const [refusal, setRefusal] = useState<string[]>([]);
  const propose = usePortalChange(name);

  const next = displayOf(lists);
  const faults = displayFaults(lists);
  // A list emptied is a change too: Publish then names it instead of staying disabled.
  const dirty = JSON.stringify(next ?? null) !== JSON.stringify(own ?? null) || Object.keys(faults).length > 0;
  const extraFault = extra !== "" && !EXTRA_KEY.test(extra);

  // A list absent from the editor is inherited: it shows the level above's, never the portal's stored one.
  const rowsOf = (list: DisplayList) => {
    const mine = lists[list];
    return mine ? completed(mine, list, fieldRows) : resolved(undefined, organization, list, fieldRows).rows;
  };
  const fromOf = (list: DisplayList): DisplayLevel => (lists[list] ? "portal" : resolved(undefined, organization, list, fieldRows).from);
  const edit = (list: DisplayList, fn: (rows: DisplayRow[]) => DisplayRow[]) => setLists({ ...lists, [list]: fn(rowsOf(list)) });
  const reset = (list: DisplayList) => {
    const rest = { ...lists };
    delete rest[list];
    setLists(rest);
  };

  const publish = () => {
    setTried(true);
    setRefusal([]);
    if (Object.keys(faults).length > 0) return;
    const appearance: Record<string, unknown> = { ...((spec.appearance as Record<string, unknown> | undefined) ?? {}) };
    if (next) appearance.display = next;
    else delete appearance.display;
    const proposed = { ...spec } as PortalSpec & { appearance?: unknown };
    if (Object.keys(appearance).length > 0) proposed.appearance = appearance as PortalSpec["appearance"];
    else delete proposed.appearance;
    propose.mutate(proposed, {
      onSuccess: (result) => {
        if (isChange(result)) setChange(result);
        setTried(false);
      },
      onError: (error) => setRefusal(linesOf(error, t("app.error.generic"))),
    });
  };

  const labelOf = (list: DisplayList, row: string) =>
    fieldLabels.get(row) ??
    (row.startsWith("extras.") ? t("portals.display.extraRow", { key: row.slice(7) }) : t(`portals.display.${list}.${row}`));

  return (
    <div className="space-y-4">
      <div>
        <h2 className="text-title font-semibold">{t("portals.display.heading")}</h2>
        <p className="text-body text-fg-muted">{t("portals.display.lead")}</p>
      </div>
      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}
      {refusal.length > 0 ? (
        <Alert tone="danger" role="alert" title={t("portals.display.refused")}>
          <p>{t("portals.display.refusedHint")}</p>
          <ul className="list-disc ps-5">
            {refusal.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {LISTS.map((list) => {
        const rows = rowsOf(list);
        const from = fromOf(list);
        const fault = tried ? faults[list] : undefined;
        return (
          <Card key={list} padding="md" className="space-y-3">
            <section aria-labelledby={`display-${list}-title`} className="space-y-3">
              <div className="flex flex-wrap items-center gap-2">
                <h3 id={`display-${list}-title`} className="min-w-0 basis-full text-body font-semibold sm:basis-auto sm:flex-1">
                  {t(`portals.display.list.${list}`)}
                </h3>
                <Badge tone={from === "portal" ? "primary" : "neutral"}>{t(`portals.display.from.${from}`)}</Badge>
                <Button size="sm" variant="secondary" disabled={!lists[list]} disabledReason={t("portals.display.why.inherited")} onClick={() => reset(list)}>
                  {t("portals.display.reset")}
                </Button>
              </div>
              <p id={`display-${list}-hint`} className="text-body text-fg-muted">
                {t(`portals.display.hint.${list}`)}
              </p>
              {fault ? (
                <InlineError>{t(fault)}</InlineError>
              ) : null}
              <ol className="divide-y divide-border rounded border border-border" aria-describedby={`display-${list}-hint`}>
                {rows.map((row, at) => (
                  <li key={row.name} className="flex flex-wrap items-center gap-2 p-2">
                    <Checkbox
                      className="min-w-0 flex-1"
                      label={labelOf(list, row.name)}
                      checked={row.shown}
                      onChange={(e) => {
                        const shown = e.currentTarget.checked;
                        edit(list, (all) => all.map((r) => (r.name === row.name ? { ...r, shown } : r)));
                      }}
                    />
                    <div className="flex gap-1">
                      <Button size="sm" variant="ghost" aria-label={t("portals.display.up", { name: labelOf(list, row.name) })} disabled={at === 0} disabledReason={t("portals.display.why.first")} onClick={() => edit(list, (all) => moved(all, at, -1))}>
                        ↑
                      </Button>
                      <Button size="sm" variant="ghost" aria-label={t("portals.display.down", { name: labelOf(list, row.name) })} disabled={at === rows.length - 1} disabledReason={t("portals.display.why.last")} onClick={() => edit(list, (all) => moved(all, at, 1))}>
                        ↓
                      </Button>
                    </div>
                    {list === "sections" && row.shown ? (
                      <details className="w-full" open={Object.values(row.heading).some((h) => h !== "") || refusal.some((line) => line.startsWith(`spec.appearance.display.dataset.sections[${at}].heading`))}>
                        <summary className="cursor-pointer text-body text-fg-muted">{t("portals.display.headingToggle")}</summary>
                        <div className="mt-2 grid gap-2 sm:grid-cols-2">
                        {languages.map((lang) => (
                          <Field key={lang} id={`display-heading-${row.name}-${lang}`} label={t("portals.display.headingIn", { lang: lang.toUpperCase() })} help={t("portals.display.hint.heading")}>
                            <Input
                              id={`display-heading-${row.name}-${lang}`}
                              value={row.heading[lang] ?? ""}
                              maxLength={120}
                              onChange={(e) => {
                                const text = e.currentTarget.value;
                                edit(list, (all) => all.map((r) => (r.name === row.name ? { ...r, heading: { ...r.heading, [lang]: text } } : r)));
                              }}
                            />
                          </Field>
                        ))}
                        </div>
                      </details>
                    ) : null}
                  </li>
                ))}
              </ol>
              {list === "about" ? (
                <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
                  <Field id="display-extra" className="min-w-0 flex-1" label={t("portals.display.extra")} help={t("portals.display.hint.extra")} errors={extraFault ? [t("portals.display.fault.extra")] : undefined}>
                    <Input id="display-extra" value={extra} maxLength={64} onChange={(e) => setExtra(e.currentTarget.value.trim())} />
                  </Field>
                  <Button
                    variant="secondary"
                    disabled={extra === "" || extraFault || rows.some((r) => r.name === `extras.${extra}`)}
                    disabledReason={t("portals.display.why.extra")}
                    onClick={() => {
                      edit("about", (all) => [...all, { name: `extras.${extra}`, shown: true, heading: {} }]);
                      setExtra("");
                    }}
                  >
                    {t("portals.display.addExtra")}
                  </Button>
                </div>
              ) : null}
            </section>
          </Card>
        );
      })}
      <Card padding="md" className={dirty ? "sticky bottom-2 z-10 flex flex-wrap items-center gap-3 shadow-md" : "flex flex-wrap items-center gap-3"}>
        <p className="min-w-0 flex-1 basis-60 text-body text-fg-muted">{dirty ? t("portals.display.unpublished") : t("portals.display.published")}</p>
        <div className="flex flex-wrap gap-2">
          <Button variant="secondary" disabled={!dirty || propose.isPending} disabledReason={t("portals.display.why.nothing")} onClick={() => setLists(ownLists(own, organization, fieldRows))}>
            {t("portals.display.discard")}
          </Button>
          <Button variant="primary" disabled={!dirty} disabledReason={t("portals.display.why.nothing")} loading={propose.isPending} onClick={publish}>
            {t("portals.display.publish")}
          </Button>
        </div>
      </Card>
    </div>
  );
}
