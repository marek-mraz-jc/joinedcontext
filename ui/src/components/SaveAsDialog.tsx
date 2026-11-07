import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../api/client";
import type { Change, Manifest } from "../api/manifest";
import { asManifests, isChange } from "../api/manifest";
import { importBody, proposeChecked } from "../api/proposal";
import { usePermissions } from "../api/permissions";
import { useProjects } from "../api/projects";
import type { components } from "../api/schema";
import { ChangeNotice } from "./ChangeNotice";
import type { ResourceTarget } from "./DeleteResourceDialog";
import { referenceManifest } from "./endpoints/sharing";
import { PermissionGuard } from "./ui/PermissionGuard";
import {
  Alert,
  Button,
  Dialog,
  Field,
  Input,
  PageFailed,
  PageLoading,
  RadioGroup,
  Select,
} from "./ui";

type ImportReport = components["schemas"]["ImportReport"];

/** What happens to the space of a resource copied into another project (T-1441). */
export type SpaceChoice = "copy" | "map" | "reference";

/** The kinds a Context Space holds, which a space copy carries with it (T-1243). */
const SPACE_CHILDREN = [
  "endpoints",
  "policies",
  "datamodels",
  "projections",
] as const;

function spaceOf(manifest: Manifest): string | undefined {
  const ref = (manifest.spec as { contextSpaceRef?: unknown } | undefined)
    ?.contextSpaceRef;
  if (typeof ref === "string") return ref;
  if (
    ref &&
    typeof ref === "object" &&
    typeof (ref as { name?: unknown }).name === "string"
  ) {
    return (ref as { name: string }).name;
  }
  return undefined;
}

/** The manifest as a copy carries it: no status, no slug (the platform mints one, EP-75). */
function copyOf(manifest: Manifest, name?: string): Manifest {
  const copy = structuredClone(manifest) as Manifest & { status?: unknown };
  delete copy.status;
  if (name) copy.metadata = { ...copy.metadata, name };
  if (copy.kind === "Endpoint") {
    const spec = { ...(copy.spec as Record<string, unknown>) };
    delete spec.slug;
    copy.spec = spec;
  }
  return copy;
}

/**
 * What one Save as sends to the import door of the target project (UI-61, T-1243, T-1441):
 * the manifests, the conflict policy, and the space mapping. A space travels with what it
 * holds and is named by the import's `rename` (MF-26); anything else keeps the name typed.
 */
export function saveAsBundle(input: {
  manifest: Manifest;
  source: string;
  target: string;
  name: string;
  choice: SpaceChoice;
  mapTo: string;
  space?: Manifest;
  children?: Manifest[];
}): {
  manifests: Manifest[];
  conflictPolicy: "fail" | "rename";
  spaceMapping: { from: string; to: string }[];
} {
  const {
    manifest,
    source,
    target,
    name,
    choice,
    mapTo,
    space,
    children = [],
  } = input;
  if (manifest.kind === "ContextSpace") {
    return {
      manifests: [copyOf(manifest), ...children.map((child) => copyOf(child))],
      conflictPolicy: "rename",
      spaceMapping: [],
    };
  }
  const copy = copyOf(manifest, name);
  const from = spaceOf(manifest);
  if (source === target || !from) {
    return { manifests: [copy], conflictPolicy: "fail", spaceMapping: [] };
  }
  if (choice === "map") {
    return {
      manifests: [copy],
      conflictPolicy: "fail",
      spaceMapping: [{ from, to: mapTo }],
    };
  }
  return {
    manifests: space ? [copyOf(space), copy] : [copy],
    conflictPolicy: "rename",
    spaceMapping: [],
  };
}

async function list(project: string, plural: string): Promise<Manifest[]> {
  const page = await unwrap(
    await api.GET("/api/v1/projects/{project}/{plural}", {
      params: { path: { project, plural } },
    }),
  );
  return asManifests(page.items ?? []);
}

/**
 * Save as (UI-61, T-1243, T-1441): the resource copied under a new name in this project, or
 * under its own name into another one, through the import door the Import page uses: checked
 * first, then proposed as one Change. Copying into another project asks what to do about its
 * space: copy it, use a space of the target, or use the source's data through a reference.
 */
export function SaveAsDialog({
  target,
  open,
  onOpenChange,
}: {
  target: ResourceTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const source = target.home ?? target.project;
  const projects = useProjects();
  const [into, setInto] = useState(source);
  const [name, setName] = useState(`${target.name}-copy`);
  const [choice, setChoice] = useState<SpaceChoice>("copy");
  const [mapTo, setMapTo] = useState("");
  const [report, setReport] = useState<ImportReport | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const mayProposeThere = usePermissions(into).can(target.kind, "propose");

  const current = useQuery({
    queryKey: ["save-as", source, target.plural, target.name],
    enabled: open,
    queryFn: async () =>
      (await unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: {
            path: { project: source, plural: target.plural, name: target.name },
          },
        }),
      )) as unknown as Manifest,
  });
  const manifest = current.data;
  const isSpace = target.kind === "ContextSpace";
  const from = manifest ? spaceOf(manifest) : undefined;
  const acrossWithSpace = !isSpace && into !== source && from !== undefined;
  const targetSpaces = useQuery({
    queryKey: ["save-as-spaces", into],
    enabled: open && acrossWithSpace,
    queryFn: () => list(into, "spaces"),
  });

  const reset = () => {
    setReport(null);
    setChange(null);
    setProblem(null);
  };

  const body = async () => {
    if (!manifest) throw new Error(t("app.loading"));
    const space =
      acrossWithSpace && choice === "copy" && from
        ? ((await unwrap(
            await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
              params: {
                path: { project: source, plural: "spaces", name: from },
              },
            }),
          )) as unknown as Manifest)
        : undefined;
    const children = isSpace
      ? (
          await Promise.all(
            SPACE_CHILDREN.map((plural) => list(source, plural)),
          )
        )
          .flat()
          .filter((child) => spaceOf(child) === target.name)
      : [];
    return saveAsBundle({
      manifest,
      source,
      target: into,
      name,
      choice,
      mapTo,
      space,
      children,
    });
  };

  const check = useMutation({
    mutationFn: async () => importBody(into, await body(), true),
    onMutate: reset,
    onSuccess: (answered) => setReport(answered as ImportReport),
    onError: (error: Error) => setProblem(error.message),
  });
  const propose = useMutation({
    mutationFn: async () => {
      if (acrossWithSpace && choice === "reference" && manifest) {
        // Outcome (c): no copy; the target uses the source's data through the endpoint (EP-15).
        return proposeChecked(
          into,
          "shared",
          referenceManifest(into, source, manifest),
          true,
        );
      }
      return importBody(into, await body(), false);
    },
    onMutate: () => setProblem(null),
    onSuccess: (answered) => {
      if (isChange(answered)) setChange(answered);
    },
    onError: (error: Error) => setProblem(error.message),
  });

  const close = (next: boolean) => {
    if (!next) {
      reset();
      setInto(source);
      setName(`${target.name}-copy`);
    }
    onOpenChange(next);
  };

  const referencing = acrossWithSpace && choice === "reference";
  const ready =
    manifest !== undefined &&
    (!acrossWithSpace || choice !== "map" || mapTo !== "");

  /** Why the target project has no space to map to, when it has none to offer. */
  const spacesProblem =
    acrossWithSpace && choice === "map"
      ? targetSpaces.isError
        ? t("saveAs.spacesUnreadable", { project: into })
        : !targetSpaces.isPending && (targetSpaces.data ?? []).length === 0
          ? t("saveAs.noSpaces", { project: into })
          : undefined
      : undefined;

  /**
   * What stands in the way of Check and Propose right now, in words (UI-44, T-1754).
   *
   * Both buttons used to be hard-`disabled` on `!ready || !mayProposeThere`, which took them out
   * of the tab order and left the only explanation in a separate `Alert` tied to neither: a slow
   * or failed GET showed a complete dialog where Check did nothing and said nothing.
   */
  const blocking = !mayProposeThere
    ? t("saveAs.mayNotPropose", { project: into })
    : current.isPending
      ? t("saveAs.reading", { name: target.label ?? target.name })
      : current.isError
        ? t("saveAs.unreadable", { name: target.label ?? target.name })
        : !ready
          ? t("saveAs.chooseSpace")
          : undefined;
  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="lg"
      title={t("saveAs.title", { name: target.label ?? target.name })}
      description={t("saveAs.lead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>
            {t("resourceDelete.close")}
          </Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            {referencing ? null : (
              <Button
                disabled={blocking !== undefined}
                disabledReason={blocking}
                loading={check.isPending}
                onClick={() => check.mutate()}
              >
                {t("saveAs.check")}
              </Button>
            )}
            <Button
              variant="primary"
              disabled={blocking !== undefined || (!referencing && report === null)}
              disabledReason={blocking ?? (referencing ? undefined : t("saveAs.checkFirst"))}
              loading={propose.isPending}
              onClick={() => propose.mutate()}
            >
              {t("saveAs.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={into} />
      ) : (
        <div className="flex flex-col gap-4">
          {problem ? (
            <Alert tone="danger" role="alert">
              {problem}
            </Alert>
          ) : null}
          {mayProposeThere ? null : (
            <Alert tone="warning">
              {t("saveAs.mayNotPropose", { project: into })}
            </Alert>
          )}
          {/* The manifest being copied has three states, not one (T-1754). `current.data` alone
              showed a complete dialog while the GET was in flight, and kept showing it for ever
              when the GET failed: the person typed a name, pressed Check, and nothing happened
              with nothing said. */}
          {current.isPending ? (
            <PageLoading label={t("saveAs.reading", { name: target.label ?? target.name })} lines={1} />
          ) : current.isError ? (
            <PageFailed error={current.error} onRetry={() => void current.refetch()} />
          ) : null}
          <Field id="save-as-project" label={t("saveAs.project")}>
            <Select
              id="save-as-project"
              value={into}
              onChange={(event) => {
                const next = event.target.value;
                setInto(next);
                // Another project keeps the name; this one needs a new one (T-1441).
                setName(next === source ? `${target.name}-copy` : target.name);
                setReport(null);
              }}
            >
              {(projects.data ?? [source]).map((project) => (
                <option key={project} value={project}>
                  {project}
                </option>
              ))}
            </Select>
          </Field>
          {isSpace ? (
            <p className="text-body text-fg-muted">{t("saveAs.spaceNamed")}</p>
          ) : (
            <Field
              id="save-as-name"
              label={t("saveAs.name")}
              description={t("saveAs.qualified", { name: `${into}/${name}` })}
            >
              <Input
                id="save-as-name"
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                  setReport(null);
                }}
              />
            </Field>
          )}
          {acrossWithSpace ? (
            <div className="flex flex-col gap-2">
              {/* The shared `RadioGroup`: one tab stop for the group, the arrow keys between the
                  options, and the `legend` naming the question — none of which the set of bare
                  radio inputs in hand-written labels that used to stand here gives (T-1754,
                  UI-16). */}
              <RadioGroup<SpaceChoice>
                name="save-as-space"
                legend={t("saveAs.space", { space: from })}
                value={choice}
                options={(
                  [
                    "copy",
                    "map",
                    ...(target.kind === "Endpoint" ? ["reference"] : []),
                  ] as SpaceChoice[]
                ).map((option) => ({ value: option, label: t(`saveAs.choice.${option}`) }))}
                onChange={(option) => {
                  setChoice(option);
                  setReport(null);
                }}
              />
              {choice === "map" ? (
                <Field
                  id="save-as-map-to"
                  label={t("saveAs.mapTo")}
                  // Read out beside the box rather than left to be guessed at: an empty list and
                  // a failed read both used to leave `mapTo` unsettable, `ready` false, and Check
                  // closed with nothing saying why (T-1754).
                  help={targetSpaces.isPending ? t("saveAs.readingSpaces") : undefined}
                  errors={spacesProblem === undefined ? undefined : [spacesProblem]}
                >
                  <Select
                    id="save-as-map-to"
                    value={mapTo}
                    disabled={targetSpaces.isPending}
                title={targetSpaces.isPending ? t("app.busyReason") : undefined}
                    onChange={(event) => {
                      setMapTo(event.target.value);
                      setReport(null);
                    }}
                  >
                    <option value="">{t("saveAs.mapTo")}</option>
                    {(targetSpaces.data ?? []).map((space) => (
                      <option
                        key={space.metadata.name}
                        value={space.metadata.name}
                      >
                        {space.metadata.name}
                      </option>
                    ))}
                  </Select>
                </Field>
              ) : null}
            </div>
          ) : null}
          {report ? (
            <section
              aria-label={t("saveAs.checked")}
              className="flex flex-col gap-1 text-caption"
            >
              <p>{t("saveAs.checked")}</p>
              {Object.entries(report.renamed ?? {}).map(([old, renamed]) => (
                <p key={old} className="font-mono">
                  {old} → {renamed}
                </p>
              ))}
              {(report.needs ?? []).length > 0 ? (
                <ul className="list-disc pl-5">
                  {(report.needs ?? []).map((need) => (
                    <li key={`${need.kind} ${need.where}`}>
                      {t(`import.report.need.${need.kind}`)}:{" "}
                      <span className="font-mono">{need.where}</span>
                    </li>
                  ))}
                </ul>
              ) : null}
            </section>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}

/** The Save as action of one row, for a person who may propose the kind (UI-61). */
export function SaveAsResourceAction({
  target,
  open: openedByRow,
  onOpenChange,
  trigger = true,
}: {
  target: ResourceTarget;
  /** The row holds the state when the action lives in its menu (T-2279). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  trigger?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [ownOpen, setOwnOpen] = useState(false);
  // The row may open this, and so may the URL (`?edit=`/`?delete=`) or the assistant's hand-off: both
  // are honoured, and closing clears both, so a page opened on one resource still opens its dialog
  // when the row owns the trigger (T-2287).
  const open = ownOpen || (openedByRow ?? false);
  const setOpen = (next: boolean) => {
    setOwnOpen(next);
    onOpenChange?.(next);
  };
  return (
    <>
      {trigger ? (
      <PermissionGuard
        project={target.home ?? target.project}
        kind={target.kind}
        verb="propose"
      >
        <Button
          size="sm"
          aria-label={t("saveAs.action", { name: target.label ?? target.name })}
          onClick={() => setOpen(true)}
        >
          {t("saveAs.button")}
        </Button>
      </PermissionGuard>
      ) : null}
      {open ? (
        <SaveAsDialog target={target} open={open} onOpenChange={setOpen} />
      ) : null}
    </>
  );
}
