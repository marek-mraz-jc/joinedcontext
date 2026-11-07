import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, whilePending } from "../../api/client";
import { readModelSource, writeModelSource } from "../../api/datamodelSource";
import type { ProblemDetails } from "../../api/client";
import { asManifests, localized, refName } from "../../api/manifest";
import { useOrgDomain, useOrgLocales } from "../../api/projects";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import {
  Alert,
  Button,
  Checkbox,
  Dialog,
  Field,
  InlineError,
  Input,
} from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { usePermissions } from "../../api/permissions";
import { takePrefill } from "../../assistant/state";
import { getDraft, putDraft } from "../../api/drafts";
import { setPageTitle } from "../../documentTitle";
import { LinkmlEditor } from "./LinkmlEditor";
import { replaceOwnSearch } from "../../assistant/HandOff";
import { spaceOfModel } from "./modelUsage";
import { mergeModels } from "./linkml";
import { ModelIdentity, setModelTitle } from "./ModelIdentity";
import { ModelFileDrop } from "./ModelFileDrop";
import { SmartDataModelsImport } from "./SmartDataModelsImport";
import { blankSource, diagnose, edit, parseModel } from "./linkml";
import { applyOperations } from "./operations";
import type { Operation } from "./operations";
import {
  diffCounts,
  proposedVersion,
  classifyChanges,
  laneOf,
  refusalToSave,
  severityOf,
} from "./breaking_detector";
import type { Lifecycle } from "./breaking_detector";

/**
 * The model editor: import a model, or edit one (DM-07, DM-13, DM-17, DM-23). The Data models
 * page opens here from its list (`?new=…`, `?edit=<name>`, T-2765); a model's Mappings live on
 * the model's own page.
 *
 * The document lives here as text and nowhere else. The shared `LinkmlEditor` edits that one
 * string and the import wizard replaces it, so the views cannot hold different models. Nothing
 * on this page writes to the platform: saving a model is a repository change, which is the
 * ordinary lane flow (CC-32, CC-63; the contract is T-0576).
 */
export interface ModelsPageProps {
  project: string;
  /** The organisation's locales, for the language maps every title needs (DM-15). */
  locales?: string[];
  /**
   * The version this edit started from. The import wizard sets it; a published model's source
   * is loaded from the portal route when name is provided.
   */
  baseline?: { source?: string; version: string; lifecycle: Lifecycle; name: string };
}


/** How long a keystroke waits before the draft is kept, as the resource form waits (UI-47). */
const DRAFT_DEBOUNCE_MS = 600;

/**
 * A free-typed or LinkML name as a manifest name: lower-case letters, digits and `-` (DM-57), a
 * camel-case word split where it turns upper-case (`AirQualityObserved` → `air-quality-observed`).
 * `undefined` when nothing is left, which is what keeps Save out of reach for an unnamed draft.
 */
function manifestName(name: string): string | undefined {
  const slug = name
    .trim()
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug === "" ? undefined : slug;
}

export function ModelsPage({
  project,
  locales: given,
  baseline,
}: ModelsPageProps): JSX.Element {
  const { t, i18n } = useTranslation();
  // Titles in the Organization's languages (T-4829): Helsinki's include fi.
  const orgLocales = useOrgLocales(project);
  const locales = given ?? orgLocales;
  // A new model's IRIs are minted under the organization's own domain (DM-13), never a guess.
  const orgDomain = useOrgDomain(project);
  // Who may not propose a model sees the drawing without edit handles (T-3589); the API refuses
  // the proposal regardless.
  const mayPropose = usePermissions(project).can("DataModel", "propose");
  // What the assistant's dock left for this page: a draft from a dropped file opens straight in
  // the editor, and a model it changed (`?edit=<name>`, AG-77) opens with its operations, which
  // are applied to the source once it is loaded (DM-13).
  const [handedOff] = useState(() => {
    const prefill = takePrefill(window.location.pathname) as { source?: unknown; operations?: unknown } | null;
    const params = new URLSearchParams(window.location.search);
    return {
      edit: params.get("edit") ?? undefined,
      // The draft this page was holding when it was last open (DM-57, T-1029).
      draft: params.get("draft") ?? undefined,
      // How a new model starts, from the list's three ways (T-2765), and the space it is for.
      start: params.get("new") ?? undefined,
      space: params.get("space") ?? undefined,
      source: typeof prefill?.source === "string" ? prefill.source : undefined,
      operations: Array.isArray(prefill?.operations) ? (prefill.operations as Operation[]) : undefined,
    };
  });
  const prefilled = handedOff.source;
  const [editing, setEditing] = useState(baseline ? undefined : handedOff.edit);
  // A new model starts in the New data model window, blank ones too (owner 2026-11-01: name it, then
  // the empty editor); a loaded or handed-off one does not.
  const [importChoice, setImportOpen] = useState<boolean | undefined>(baseline || prefilled || editing ? false : undefined);
  const [chosen, setChosen] = useState(baseline);
  const [breakingConfirmed, setBreakingConfirmed] = useState(false);
  /** What a second import could not take because the model already had it (T-1102). */
  const [importConflicts, setImportConflicts] = useState<string[]>([]);
  // What the person typed; before the first keystroke the source is the loaded or blank one.
  const [edited, setEdited] = useState<string | undefined>(baseline?.source ?? prefilled);
  const [saving, setSaving] = useState(false);
  const [checking, setChecking] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [checkInfo, setCheckInfo] = useState<{ severity: string; version: string } | null>(null);
  const [changeNotice, setChangeNotice] = useState<Change | null>(null);
  const [nameChoice, setNewName] = useState<string | undefined>(undefined);
  /** The shared draft this page last kept: its version for the next keep (AG-61), and what it held. */
  const [kept, setKept] = useState<{ name: string; version: number; source: string; space: string } | null>(null);
  const [heldConflict, setHeldConflict] = useState(false);

  // A model imported or inferred here lives in this component until Save proposes it, and a
  // reload used to lose it. The Portal already shares drafts for exactly this (AG-61), so an
  // unpublished model is kept there under its own name and `?draft=` brings it back. What it
  // holds is a fallback, never an assignment: the first keystroke wins over it.
  const heldQuery = useQuery({
    queryKey: ["datamodel-draft", project, handedOff.draft],
    enabled: Boolean(handedOff.draft) && !baseline && prefilled === undefined,
    retry: false,
    queryFn: () => getDraft(project, "DataModel", handedOff.draft ?? ""),
  });
  const held = useMemo(() => {
    const draft = heldQuery.data;
    const spec = (draft?.manifest as { spec?: { linkml?: unknown; space?: unknown } } | undefined)?.spec;
    return draft && typeof spec?.linkml === "string"
      ? {
          name: draft.name,
          version: draft.version,
          source: spec.linkml,
          space: typeof spec.space === "string" ? spec.space : "",
        }
      : undefined;
  }, [heldQuery.data]);

  const newName = nameChoice ?? held?.name ?? "";
  // The editor names no space (T-6162, DM-57): a model started from a space's page is that
  // space's model, any other one a project model a space adopts later (DM-75).
  const newSpace = handedOff.space ?? held?.space ?? "";
  // A draft the address names opens as it was kept, never behind the window while it is read.
  const importOpen = importChoice ?? !(held || handedOff.draft);

  // The list keys are shared with every page that lists these kinds, so the cache holds the list
  // as the API answers it and the manifests are read off it here (T-0625).
  const listOf = async (plural: string) =>
    unwrap(
      await api.GET("/api/v1/projects/{project}/{plural}", {
        params: { path: { project, plural } },
      }),
    );
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    retry: false,
    queryFn: () => listOf("datamodels"),
    select: (list) => asManifests(list.items ?? []),
  });
  const opened = useMemo((): ModelsPageProps["baseline"] => {
    const spec = models.data?.find((model) => model.metadata.name === editing)?.spec as
      | { version?: unknown; lifecycle?: unknown }
      | undefined;
    return editing && spec
      ? {
          name: editing,
          version: typeof spec.version === "string" ? spec.version : "0.1.0",
          lifecycle: (typeof spec.lifecycle === "string" ? spec.lifecycle : "draft") as Lifecycle,
        }
      : undefined;
  }, [models.data, editing]);
  const published = chosen ?? opened;

  const activeModelName = published?.name ?? baseline?.name ?? editing;
  // A draft nobody has published yet — inferred from a file, or started from blank — has no
  // manifest, so saving it creates one: it needs a name and the space it belongs to (DM-57).
  const creating = activeModelName === undefined;
  const targetName = activeModelName ?? manifestName(newName);
  const taken =
    creating && (models.data ?? []).some((manifest) => manifest.metadata.name === targetName);


  // A published model's source lives in the repository and is read through DM-56's route.
  const loaded = useQuery({
    queryKey: ["datamodel-source", project, activeModelName],
    enabled: Boolean(activeModelName) && !baseline?.source,
    queryFn: () => readModelSource(project, activeModelName ?? ""),
  });
  const loadingSource = loaded.isLoading;
  const loadError = loaded.error instanceof Error ? loaded.error.message : null;
  const handedOperations = editing === undefined ? undefined : handedOff.operations;
  const applied = useMemo(
    () =>
      handedOperations && loaded.data !== undefined ? applyOperations(loaded.data, handedOperations) : undefined,
    [handedOperations, loaded.data],
  );
  const source =
    edited ?? applied?.source ?? held?.source ?? loaded.data ?? blankSource(orgDomain, "new-model");
  const setSource = setEdited;
  const publishedSource = published?.source ?? loaded.data;

  const model = useMemo(() => parseModel(source), [source]);
  // The LinkML title first (a rename writes it), then the title the manifest carries, as the
  // list shows it: a model titled only in its manifest is not "Unnamed" (uijudge T-6162).
  const manifestTitle = models.data?.find((one) => one.metadata.name === activeModelName)?.metadata.title as
    | string
    | Record<string, string>
    | undefined;
  const displayName = creating
    ? newName
    : (model.title?.en ?? Object.values(model.title ?? {})[0] ?? localized(manifestTitle, i18n.language, ""));
  // A rename writes the model's title; a new model's id follows the name until it is saved.
  const rename = (name: string) => {
    if (name.trim() !== "") {
      setSource(edit(source, (document) => setModelTitle(document, name.trim())));
    }
    if (creating) {
      setNewName(name);
    }
  };

  // What is kept: the manifest the Save would propose, minus the name the person may still
  // change. A model the project already publishes is not kept — its source lives in the
  // repository and the lane is how it changes — so this is the inferred and the imported one
  // (T-1029, T-1105), whichever tab produced it.
  const unpublished =
    models.isSuccess &&
    targetName !== undefined &&
    !(models.data ?? []).some((manifest) => manifest.metadata.name === targetName);
  // Kept only when it differs from what the draft holds: each keep moved the version, and a keep
  // keyed on the version alone wrote the same draft again every DRAFT_DEBOUNCE_MS (T-5066).
  const lastKept = kept?.name === targetName ? kept : held;
  const keeping =
    unpublished &&
    edited !== undefined &&
    !heldConflict &&
    (source !== lastKept?.source || newSpace !== (lastKept?.space ?? ""));
  const keptVersion = [kept, held].find(
    (candidate) => candidate !== null && candidate !== undefined && candidate.name === targetName,
  )?.version;
  useEffect(() => {
    if (!keeping) {
      return;
    }
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const answer = await putDraft(
            project,
            "DataModel",
            targetName,
            {
              kind: "DataModel",
              metadata: { name: targetName },
              spec: { linkml: source, ...(newSpace ? { space: newSpace } : {}) },
            },
            keptVersion,
          );
          setKept({ name: targetName, version: answer.version, source, space: newSpace });
          // The page's own address (T-3239): a bare replaceState read as an assistant hand-off
          // and mounted the page afresh, closing the editor view the person had open (T-5066).
          const query = new URLSearchParams(window.location.search);
          if (query.get("draft") !== targetName) {
            query.set("draft", targetName);
            replaceOwnSearch(query);
          }
        } catch (err: unknown) {
          // Somebody else has this draft open and saved first; keeping ours would overwrite
          // theirs unseen, so the page stops keeping and says so (UI-48).
          if ((err as { status?: number }).status === 409) {
            setHeldConflict(true);
          }
        }
      })();
    }, DRAFT_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [keeping, project, targetName, source, newSpace, keptVersion]);

  const changes = useMemo(
    () => (publishedSource ? classifyChanges(parseModel(publishedSource), model) : []),
    [publishedSource, model],
  );
  const severity = severityOf(changes);
  // Nothing to propose until the text moves away from the version it started from (T-4830).
  const moved = publishedSource === undefined || publishedSource.trim() !== source.trim();
  const nextVersion = published ? proposedVersion(published.version, changes, moved) : "1.0.0";
  const counts = diffCounts(changes);
  const lane = laneOf(changes, published?.lifecycle ?? "draft");
  const refusal = published
    ? refusalToSave(changes, published.version, published.version)
    : undefined;

  // Who breaks when this model's major changes (DM-25). A space names its model, and an
  // endpoint names its space, so the consumers of a model are the endpoints behind it.
  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    retry: false,
    queryFn: () => listOf("spaces"),
    select: (list) => asManifests(list.items ?? []),
  });
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    retry: false,
    queryFn: () => listOf("endpoints"),
    select: (list) => asManifests(list.items ?? []),
  });

  // One model per space (DM-61): a space that has one already is named, and a new model is not
  // offered it. The space's own pointer and the models that name it both count.
  const occupied = useMemo(() => {
    const held = new Map<string, string>();
    for (const one of models.data ?? []) {
      const space = spaceOfModel(one, spaces.data ?? []);
      if (space !== undefined && one.metadata.name !== activeModelName) {
        held.set(space, one.metadata.name);
      }
    }
    return held;
  }, [models.data, spaces.data, activeModelName]);
  const spaceTakenBy = creating && newSpace ? occupied.get(newSpace) : undefined;
  // Why the New data model window cannot create yet, said on its button.
  const newNameRefusal = !newName.trim()
    ? t("models.nameFirst")
    : taken
      ? t("models.create.taken", { name: targetName })
      : spaceTakenBy
        ? t("models.create.spaceTaken", { space: newSpace, model: spaceTakenBy })
        : undefined;

  const consumers = useMemo(() => {
    const name = published?.name ?? model.name;
    if (!name) {
      return [];
    }
    const servingSpaces = (spaces.data ?? [])
      .filter((space) => refName((space.spec as { dataModelRef?: unknown })?.dataModelRef) === name)
      .map((space) => space.metadata.name);
    const servingEndpoints = (endpoints.data ?? [])
      .filter((endpoint) =>
        servingSpaces.includes(
          refName((endpoint.spec as { contextSpaceRef?: unknown })?.contextSpaceRef),
        ),
      )
      .map((endpoint) => `Endpoint/${endpoint.metadata.name}`);
    return [...servingSpaces.map((space) => `ContextSpace/${space}`), ...servingEndpoints];
  }, [spaces.data, endpoints.data, published, model.name]);

  // T-1102: a second import joins the model being edited instead of replacing it, so a Vehicle
  // and an AirQualityObserved can sit in one model and a slot can relate them. A name the model
  // already has is kept and reported: what is in hand may have been edited, and an import must
  // not undo that.
  // Types join the model from a Smart Data Model or a spreadsheet (owner 2026-11-01): into a model
  // with types the import merges and keeps what is there; into an empty one it is the model.
  const [addFrom, setAddFrom] = useState<"sdm" | "file" | null>(null);
  const takeTypes = (imported: string) => {
    const editing = source.trim() !== "" && parseModel(source).classes.length > 0;
    const { source: joined, conflicts } = editing
      ? mergeModels(source, imported)
      : { source: imported, conflicts: [] };
    setSource(joined);
    setImportConflicts(conflicts.map((conflict) => `${conflict.section}: ${conflict.name}`));
    // A Smart Data Model or a sheet brings types; the model keeps the name given in the new-model
    // window (owner 2026-11-01).
    if (!editing) {
      setChosen(undefined);
      setEditing(undefined);
    }
    setAddFrom(null);
  };
  const onImport = (imported: string) => takeTypes(imported);

  // The space travels with a create, and with nothing else: for a model that exists the route
  // reads the space off its manifest (DM-56).
  const writeSource = (dryRun: boolean) =>
    writeModelSource({
      project,
      name: targetName ?? "",
      source,
      dryRun,
      space: creating && newSpace ? newSpace : undefined,
    });

  // The detail says what happened and the errors say where: a model refused for its
  // relationships names each broken rule there (DM-68), so both are shown.
  const refusedBecause = (status: number, problem: Partial<ProblemDetails>): string => {
    const errors = problem.errors && problem.errors.length > 0 ? problem.errors.join("; ") : undefined;
    if (problem.detail && errors) return `${problem.detail}: ${errors}`;
    return problem.detail ?? errors ?? t("models.source.refused", { reason: problem.title ?? `HTTP ${status}` });
  };

  // An error the editor lists blocks Save (DM-68); the server refuses the same list regardless.
  const blocking = useMemo(
    () => diagnose(source, locales).filter((diagnostic) => diagnostic.severity === "error").length,
    [source, locales],
  );

  const handleCheck = async () => {
    if (!targetName) return;
    setChecking(true);
    setSaveError(null);
    setCheckInfo(null);
    try {
      const answer = await writeSource(true);
      if (answer.kind === "checked") {
        setCheckInfo({ severity: answer.result.severity, version: answer.result.version });
      } else if (answer.kind === "refused") {
        setSaveError(refusedBecause(answer.status, answer.problem));
      }
    } catch (err: unknown) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setChecking(false);
    }
  };

  const handleSave = async () => {
    if (!targetName) return;
    setSaving(true);
    setSaveError(null);
    try {
      const answer = await writeSource(false);
      if (answer.kind === "proposed") {
        setChangeNotice(answer.change);
      } else if (answer.kind === "refused") {
        setSaveError(refusedBecause(answer.status, answer.problem));
      }
    } catch (err: unknown) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  // The tab names the model the editor holds; the visible heading is the model's name (ModelIdentity).
  useEffect(() => {
    setPageTitle([displayName || t("models.editor.title"), project]);
  }, [displayName, project, t]);

  // Why Propose cannot be pressed now, in the order a person fixes it; undefined when it can.
  const proposeBlocked =
    saving || checking
      ? t("app.busyReason")
      : taken
        ? t("models.source.nameTaken")
        : blocking > 0
          ? t("models.source.fixErrors", { count: blocking })
          : !moved
            ? t("models.source.unchanged")
            : severity === "breaking" && !breakingConfirmed
              ? t("models.source.confirmFirst", { version: nextVersion })
              : undefined;
  return (
    <div className="flex flex-col gap-4">
      {/* One row: the model's name, Rename and version, then Check, Propose and Remove;
          the breadcrumb leads back to the list (owner 2026-10-31). */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <ModelIdentity
          name={displayName}
          id={targetName}
          creating={creating}
          version={published?.version}
          readOnly={!mayPropose}
          onRename={rename}
        />
        <div className="flex flex-wrap items-center gap-2">
          {
          targetName && !spaceTakenBy ? (
            <>
              {/* Refused where they stand for a person who may not propose (UI-44, T-2848). */}
              <PermissionGuard project={project} kind="DataModel" verb="propose">
                <Button
                  size="sm"
                  onClick={handleCheck}
                  disabled={checking || saving || taken}
                  disabledReason={taken ? t("models.source.nameTaken") : t("app.busyReason")}
                >
                  {t("models.source.saveCheck")}
                </Button>
              </PermissionGuard>
              {severity === "breaking" ? (
                <Checkbox
                  label={t("models.source.confirmBreaking", { version: nextVersion })}
                  checked={breakingConfirmed}
                  onChange={(event) => setBreakingConfirmed(event.target.checked)}
                />
              ) : null}
              <PermissionGuard project={project} kind="DataModel" verb="propose">
                <Button
                  size="sm"
                  variant="primary"
                  onClick={handleSave}
                  disabled={proposeBlocked !== undefined}
                  disabledReason={proposeBlocked}
                >
                  {t("models.source.save", { version: nextVersion })}
                </Button>
              </PermissionGuard>
              {published ? (
                <DeleteResourceAction
                  target={{ project, kind: "DataModel", plural: "datamodels", name: published.name }}
                />
              ) : null}
              {/* The reason a disabled Propose gives, in the open as the row menu shows it: a pale
                  button alone read as broken (T-6254 judge). */}
              {/* Only the reasons not already shown elsewhere: errors have their list, a taken name its alert. */}
              {proposeBlocked && !saving && !checking && !taken && blocking === 0 ? (
                <p className="basis-full text-right text-caption text-fg-muted max-sm:text-left">{proposeBlocked}</p>
              ) : null}
            </>
          ) : null}
        </div>
      </div>

      {loadingSource ? (
        <p role="status" className="text-body text-fg-muted">
          {t("models.source.loading")}
        </p>
      ) : null}

      {changeNotice ? <ChangeNotice change={changeNotice} project={project} /> : null}

      {applied && applied.refused.length > 0 ? (
        <Alert role="alert" tone="danger">
          <p>{t("models.source.handOffRefused")}</p>
          <ul className="mt-1 list-disc pl-5">
            {applied.refused.map((refusal) => (
              <li key={refusal.index}>{refusal.reason}</li>
            ))}
          </ul>
        </Alert>
      ) : null}

      {blocking > 0 && targetName ? (
        <p className="text-body text-danger">{t("models.source.fixErrors", { count: blocking })}</p>
      ) : null}

      {saveError || loadError ? (
        <Alert role="alert" tone="danger">
          {saveError ?? loadError}
        </Alert>
      ) : null}

      {checkInfo ? (
        <Alert tone="info">
          <span className="font-medium">
            {t(`models.source.severity.${checkInfo.severity}`)} · {t("models.source.willBecome", { version: checkInfo.version })}
          </span>
        </Alert>
      ) : null}

      {/* Nothing to say until the text moved: no "No change" box (owner 2026-10-31). */}
      {published && (moved || changes.length > 0 || refusal) ? (
        <section
          aria-labelledby="models-changes"
          className={
            severity === "breaking"
              ? "rounded border border-danger p-3"
              : "rounded border border-border p-3"
          }
        >
          <h2 id="models-changes" className="text-sm font-semibold">
            {t(`models.severity.${severity}`)} · {t(`lane.${lane}`)}
          </h2>
          {moved ? (
            <p className="mt-1 text-body">
              {t("models.source.diffSummary", { ...counts, from: published.version, to: nextVersion })}
            </p>
          ) : null}
          {refusal ? (
            <InlineError className="mt-1">
              {refusal}
            </InlineError>
          ) : null}
          {changes.length > 0 ? (
            <ul className="mt-2 flex flex-col gap-1 text-sm">
              {changes.map((change) => (
                <li key={`${change.subject}-${change.reason}`}>
                  <span className="font-mono text-xs">{change.subject}</span> {change.reason}
                </li>
              ))}
            </ul>
          ) : (
            <p className="mt-1 text-body text-fg-muted">{t("models.noChanges")}</p>
          )}
          {severity === "breaking" && consumers.length > 0 ? (
            <p className="mt-2 text-sm">
              {t("models.consumers", { consumers: consumers.join(", ") })}
            </p>
          ) : null}
        </section>
      ) : null}

      {/* New data model (owner 2026-11-01): one window for its name, then the empty editor. Types
          come in from the editor's Add type: blank, a Smart Data Model or a spreadsheet. */}
      <Dialog
        open={importOpen}
        onOpenChange={setImportOpen}
        title={t("models.newDialog.title")}
        description={t("models.newDialog.description")}
        closeLabel={t("models.importDialog.close")}
        size="md"
      >
        <form
          className="flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            if (!newNameRefusal) setImportOpen(false);
          }}
        >
          <Field id="new-model-name" label={t("models.identity.name")} help={t("models.create.nameHint")}>
            <Input id="new-model-name" value={newName} onChange={(event) => setNewName(event.target.value)} />
          </Field>
          {/* Said in the window, where the name is typed: a taken name never reaches the editor. */}
          {taken ? <InlineError>{t("models.create.taken", { name: targetName })}</InlineError> : null}
          {spaceTakenBy ? <InlineError>{t("models.create.spaceTaken", { space: newSpace, model: spaceTakenBy })}</InlineError> : null}
          <div className="flex flex-wrap items-center justify-end gap-3 border-t border-border pt-3">
            {/* Why Create waits, where a sighted person reads it; a taken name says so above. */}
            {!newName.trim() ? <p className="mr-auto text-caption text-fg-muted">{t("models.nameFirst")}</p> : null}
            <Button
              type="submit"
              variant="primary"
              disabled={Boolean(newNameRefusal)}
              disabledReason={newNameRefusal}
            >
              {t("models.newDialog.create")}
            </Button>
          </div>
        </form>
      </Dialog>
      <Dialog
        open={addFrom !== null}
        onOpenChange={(open) => setAddFrom(open ? addFrom : null)}
        title={t(addFrom === "file" ? "models.addFrom.fileTitle" : "models.importDialog.title")}
        closeLabel={t("app.close")}
        size="lg"
      >
        {addFrom === "file" ? (
          <ModelFileDrop project={project} onPopulate={(draft) => takeTypes(draft)} />
        ) : (
          <SmartDataModelsImport onImport={onImport} />
        )}
      </Dialog>

      <div className="flex flex-col gap-3">
        {importConflicts.length > 0 ? (
          <Alert tone="info" role="status">
            {t("models.sdm.mergedKept", { names: importConflicts.join(", ") })}
          </Alert>
        ) : null}
        {creating && (taken || spaceTakenBy || heldConflict || kept || held) ? (
          <section aria-label={t("models.create.title")} className="flex flex-col gap-2">
            {taken ? (
              <InlineError>
                {t("models.create.taken", { name: targetName })}
              </InlineError>
            ) : null}
            {spaceTakenBy ? (
              <InlineError>
                {t("models.create.spaceTaken", { space: newSpace, model: spaceTakenBy })}
              </InlineError>
            ) : null}
            {heldConflict ? (
              <InlineError>
                {t("models.create.heldConflict")}
              </InlineError>
            ) : kept ?? held ? (
              <p role="status" className="text-body text-fg-muted">
                {t("models.create.held", { name: (kept ?? held)?.name })}
              </p>
            ) : null}
          </section>
        ) : null}
        <LinkmlEditor
          source={source}
          onChange={setSource}
          locales={locales}
          onImport={setAddFrom}
          readOnlyDrawing={!mayPropose}
        />
      </div>
    </div>
  );
}
