import { Suspense, useEffect, useId, useMemo, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import validator from "./forms/validator";
import { errorMessageKey } from "./forms/errorMessages";
import { SchemaForm } from "./forms/LazySchemaForm";
import { namesOfRefs } from "../schemas/pickers";
import type { JsonSchema, ManifestSource, UiSchema } from "./forms/types";
import { Alert, Button, Checkbox, deniedReason as refusal, Tabs, tabPanelProps, PageLoading } from "./ui";
import { FormFrame } from "./forms/FormRoute";
import type { DialogSize } from "./ui";
import { guideUrl, useBranding } from "../branding";
import { digestOf } from "../api/drafts";
import type { Verdict } from "../api/drafts";
import { ResourceHistory } from "./ResourceHistory";
import { standingIn } from "../assistant/state";
import { usePermissions } from "../api/permissions";
import { extractName, manifestOf } from "./forms/resourceForm";
import { useFormArrangement } from "./forms/useFormArrangement";
import { useResourceDraft } from "./forms/useResourceDraft";
import { useDryRunCheck } from "./forms/useDryRunCheck";
import { useYamlEditor } from "./forms/useYamlEditor";
import { useYamlSubmitQueue } from "./forms/useYamlSubmitQueue";
import { ResourceFormFooter, verdictStateOf } from "./forms/ResourceFormFooter";
import type { ResourceFormFooterProps, VerdictState } from "./forms/ResourceFormFooter";
import { FillFromSentence, FormGuideLink, ResourceFormNotes } from "./forms/ResourceFormNotes";
import { ResourceYamlPanel } from "./forms/ResourceYamlPanel";

export type { ManifestSource };

export interface ResourceFormDialogProps<T> {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  schema: JsonSchema;
  /**
   * The manifest kind this form edits, e.g. `Endpoint`: what lets a
   * `portal/forms/{kind lowercased}.uischema.yaml` arrange it (UI-02). Defaults to `draftKind`, so
   * a form that names its draft is arranged by its kind too (T-2754). Without either, or for a
   * kind nobody wrote a manifest for, the form renders from `uiSchema` alone.
   */
  kind?: string;
  uiSchema?: UiSchema;
  /**
   * The name the resource already has. A manifest is its path, so a rename in a form is a new
   * resource at a new name and the old one left behind (MF-11, T-0796): with this set the name
   * field is read-only and a submit that changed it anyway — through the YAML view — is refused.
   */
  lockedName?: string;
  formData?: T;
  submitLabel: string;
  disabled?: boolean;
  /** Why proposing is closed right now (PL-49): both views disable their button and say so. */
  submitDisabledReason?: string;
  /**
   * Why the form cannot be checked as it stands, and the field that would fix it: Check sends
   * nothing, says this beside the button and brings that field into view (T-3969). A kind whose
   * form holds more than its manifest can say passes it rather than check a smaller manifest.
   */
  checkRefusal?: { message: string; field?: string };
  /**
   * A proposal is in flight. The form is closed for the duration and the button says so, which
   * `disabled` alone cannot: a disabled button is one that cannot be pressed for any reason,
   * and a person who clicked and sees nothing move clicks again (UI-01, T-0962).
   */
  submitting?: boolean;
  error?: string | null;
  size?: DialogSize;
  /** Adds the YAML view of the manifest beside the form; both edit the same data. */
  source?: ManifestSource<T>;
  /** Rendered above the form: the slug generator, a quota warning, whatever the kind needs. */
  children?: ReactNode;
  /** Rendered under the last field, above the submit line: panels that follow the fields. */
  afterFields?: ReactNode;
  project?: string;
  draftKind?: string;
  draftName?: string;
  verdict?: Verdict | null;
  onVerdictChange?: (verdict: Verdict | null) => void;
  /**
   * Runs the kind's check on what the dialog holds (PF-57): a Check beside the verdict chip in
   * both views, so a manifest pasted in the YAML view is checked where it was typed (T-0884).
   */
  onCheck?: (data: T) => void;
  /**
   * The kind's collection, so a dialog with no `onCheck` of its own can still run the check
   * (`POST .../{plural}?dryRun=All`). Under strict validation a form that saves a draft cannot
   * propose without a green verdict, so every such form needs a Check (T-0779, PF-57).
   */
  plural?: string;
  onSubmit: (data: T, draft?: { kind: string; name: string }) => void;
  onChange?: (data: T | undefined) => void;
  /**
   * Runs the check by itself once the form has rested `AUTO_CHECK_MS` after a change (T-4818), so
   * the verdict Propose waits for comes without a hidden step. A form with an empty required
   * field, or one the dialog refuses to check, is not sent: its fields already say why.
   */
  autoCheck?: boolean;
}

type View = "form" | "yaml" | "history";

const VIEWS: View[] = ["form", "yaml"];

/**
 * Strict validation refuses a proposal whose draft carries no fresh green verdict, and the click
 * saves that draft first (T-0769), so the button says so before the first save, not after a click
 * that proposed nothing (T-0779).
 */
const PROPOSE_REASON: Partial<Record<VerdictState, string>> = {
  none: "drafts.proposeReason.none",
  red: "drafts.proposeReason.red",
  stale: "drafts.proposeReason.stale",
};

/** One frame for every manifest form: the schema decides the fields, the caller the kind. */
export function ResourceFormDialogBody<T>({
  open,
  onOpenChange,
  title,
  description,
  schema,
  kind: ownKind,
  uiSchema,
  lockedName,
  formData,
  submitLabel,
  disabled: closed,
  submitDisabledReason,
  checkRefusal,
  submitting,
  error,
  size = "xl",
  source,
  children,
  afterFields,
  project,
  draftKind,
  draftName,
  verdict: externalVerdict,
  onCheck,
  plural,
  onVerdictChange,
  onSubmit,
  onChange,
  autoCheck = false,
}: ResourceFormDialogProps<T>): JSX.Element {
  const { t } = useTranslation();
  const branding = useBranding();
  const viewsId = useId();
  const advancedHintId = useId();
  const kind = ownKind ?? draftKind;
  // A role that may not propose the kind reads the form and is told why it cannot send it, as
  // `PermissionGuard` says it of a button (UI-44, PF-51); the API refuses the same proposal anyway.
  const { can, isLoading: permissionsLoading } = usePermissions(project ?? "");
  const deniedReason =
    kind && project && !can(kind, "propose") ? refusal(t, project, "propose", kind) : undefined;
  // A proposal in flight closes the form as surely as a caller's own `disabled` does; what it
  // adds is that the button says which of the two it is (T-0962).
  const disabled = closed || submitting === true || deniedReason !== undefined;

  const arrangement = useFormArrangement({ open, kind, schema, uiSchema, lockedName, project, orgDomain: branding.orgDomain });
  const isLax = (branding as { validation?: string })?.validation === "lax";
  // The base comes from the installation's branding, which an administrator writes, so a
  // `javascript:` there would otherwise reach the anchor below (T-2252, T-2409). It is checked
  // in `ExternalLink`, which is where every link out of the Portal is checked — and which keeps
  // the words when the address is not a place, instead of dropping the sentence.
  const guideHref = guideUrl(branding, arrangement.guide);

  const [view, setView] = useState<View>("form");
  const yaml = useYamlEditor(source);
  const currentDigest = useMemo(() => digestOf(formData ? manifestOf(formData, source) : {}), [formData, source]);
  const draft = useResourceDraft({
    open,
    project,
    draftKind,
    draftName,
    formData,
    source,
    currentDigest,
    verdict: externalVerdict,
    onVerdictChange,
    onChange,
  });
  const { activeName } = draft;

  // The assistant is told which form is open and which draft it edits, so a question asked from
  // here is answered about this form (T-1611, UI-61). No values travel: the draft is where they are.
  useEffect(() => {
    if (!open || !kind) {
      return;
    }
    standingIn({ kind, name: activeName || undefined });
    return () => {
      standingIn(null);
    };
  }, [open, kind, activeName]);

  /**
   * A form can be checked when the page gives the dialog a check of its own or the collection to
   * dry run. It is the same set of forms whose proposal names its draft, so it is also the set
   * the strict gate applies to: a form that cannot be checked is never refused for the verdict
   * it has no way to get (T-0779).
   */
  const canCheck = Boolean(onCheck) || Boolean(plural && project && draftKind);
  const check = useDryRunCheck({
    kind,
    schema,
    formData,
    formView: view === "form",
    currentDigest,
    verdict: draft.verdict,
    updateVerdict: draft.updateVerdict,
    canCheck,
    disabled,
    checkRefusal,
    onCheck,
    autoCheck,
    source,
    project,
    plural,
    draftKind,
    draftName,
    onChange,
    readYaml: yaml.read,
    onYamlChecked: () => yaml.setIssues([]),
    advancedFolded: arrangement.advancedFields && !arrangement.advanced,
    showAdvanced: () => arrangement.setAdvanced(true, false),
    advanced: arrangement.advanced,
  });

  const verdictState = verdictStateOf(draft.verdict, currentDigest);
  const proposeKey = canCheck && draftKind && activeName ? PROPOSE_REASON[verdictState] : undefined;
  const proposeReason = proposeKey ? t(proposeKey) : undefined;
  const effectiveSubmitDisabledReason =
    deniedReason ?? (isLax ? submitDisabledReason : submitDisabledReason || proposeReason);

  /** A dialog opens on its form again, whatever view it was closed from. */
  function handleOpenChange(next: boolean) {
    if (!next) {
      setView("form");
      yaml.reset();
      check.reset();
      draft.reset();
    }
    onOpenChange(next);
  }

  function show(next: View) {
    if (next === view || !source) {
      return;
    }
    let form = formData;
    if (view === "yaml") {
      const read = yaml.read();
      if (read === null) {
        return;
      }
      onChange?.(read);
      form = read;
    }
    if (next === "yaml") {
      yaml.showForm(form ?? ({} as T));
    }
    yaml.setIssues([]);
    setView(next);
  }

  // An existing resource shows its own history beside the form and the YAML (T-3975).
  const views: View[] = project && draftKind && lockedName ? [...VIEWS, "history"] : VIEWS;
  const [restored, setRestored] = useState<string | null>(null);

  /**
   * The draft is saved before the proposal names it, and that save is a request of its own: until
   * it answers the page's `submitting` has not started, so a second click proposed twice
   * (UI-48, T-1753). The ref refuses the second click; the state shows the button busy.
   */
  const savingRef = useRef(false);
  const [saving, setSaving] = useState(false);

  function handleSubmit(form: T) {
    // The YAML view edits `metadata.name` freely, so the refusal is here and not only in the
    // read-only field above it (T-0796).
    if (lockedName !== undefined && extractName(form) !== lockedName) {
      draft.setConflict(t("resourceEdit.renamed", { name: lockedName }));
      return;
    }
    if (savingRef.current || submitting) {
      return;
    }
    savingRef.current = true;
    setSaving(true);
    draft
      .draftRefFor(form)
      .then(
        (draftRef) => onSubmit(form, draftRef),
        () => draft.setConflict(t("drafts.conflict")),
      )
      .finally(() => {
        // One task later, not now: the page's mutation tells its observers on the next task, and
        // until then `submitting` still reads false and would let a second click through.
        setTimeout(() => {
          savingRef.current = false;
          setSaving(false);
        }, 0);
      });
  }

  // The YAML view submits what the form view would, judged by the schema of the render after
  // `onChange` (T-0890): the parent may widen it for what was typed.
  const enqueueYaml = useYamlSubmitQueue<T>((form) => {
    const { errors } = validator.validateFormData(namesOfRefs(kind, form), schema);
    if (errors.length > 0) {
      yaml.setIssues(errors.map((issue) => `${issue.property ?? ""} ${t(errorMessageKey(issue))}`.trim()));
      return;
    }
    yaml.setIssues([]);
    handleSubmit(form);
  });
  function submitYaml() {
    const form = yaml.read();
    if (form === null) {
      return;
    }
    onChange?.(form);
    enqueueYaml(form);
  }

  // What the footer's buttons answered, beside them, where the person is looking (T-1424): the
  // page's own refusal of a check or a proposal, or this dialog's own check refused.
  const footer: ResourceFormFooterProps = {
    onCancel: () => handleOpenChange(false),
    check: canCheck
      ? {
          onClick: check.runCheck,
          disabled: disabled || check.pending,
          disabledReason: deniedReason ?? (submitDisabledReason || t("app.busyReason")),
        }
      : undefined,
    stoppedMessage: check.stoppedMessage,
    verdict:
      draftKind && activeName
        ? { verdict: draft.verdict, state: verdictState, refused: verdictState === "none" && check.refusedWith(error) }
        : undefined,
    proposeReason,
    lax: isLax,
    error: error ?? check.error,
  };

  // A person who may not create the kind is told so at the top, in words, instead of a whole
  // form greyed out with the reason in API terms under its last field (T-4165, audit T-4158).
  if (deniedReason !== undefined && lockedName === undefined && !permissionsLoading) {
    return (
      <FormFrame
        open={open}
        onOpenChange={handleOpenChange}
        title={title}
        description={description}
        size={size}
        closeLabel={t("resourceDelete.close")}
      >
        <Alert role="status" tone="info">
          {t("form.mayNotCreate")}
        </Alert>
      </FormFrame>
    );
  }

  return (
    // A page at its own address where a route hosts it, the dialog elsewhere (T-2474).
    <FormFrame
      open={open}
      onOpenChange={handleOpenChange}
      title={title}
      description={description}
      size={size}
      // Not "Cancel": the footer has a Cancel of its own, and two buttons of one name are two
      // things a screen reader cannot tell apart (UI-15, T-1753).
      closeLabel={t("resourceDelete.close")}
      help={<FormGuideLink kind={kind} href={guideHref} />}
    >
      <div className="flex flex-col gap-4">
        <ResourceFormNotes
          conflict={draft.conflict}
          saveRefused={draft.saveRefused}
          project={project}
          draftKind={draftKind}
          draft={draft.currentDraft}
          verdict={draft.verdict}
          restartsStream={check.restartsStream}
        />

        {source || kind ? (
          <div className="flex flex-wrap items-center justify-between gap-2">
            {source ? (
              <Tabs
                id={viewsId}
                label={title}
                variant="pill"
                tabs={views.map((name) => ({ value: name, label: t(`form.view.${name}`) }))}
                value={view}
                onChange={show}
              />
            ) : null}
            {kind ? <FillFromSentence kind={kind} /> : null}
          </div>
        ) : null}

        {view === "form" ? (
          <div {...(source ? tabPanelProps(viewsId, "form") : {})} className="flex flex-col gap-4">
            {restored ? (
              <Alert role="status" tone="info" data-testid="history-restored">
                {t("resourceHistory.loaded", { id: restored })}
              </Alert>
            ) : null}
            {children ? (
              <div className="flex flex-col gap-3">{children}</div>
            ) : null}

            <Suspense fallback={<PageLoading label={t("app.loading")} lines={2} />}>
            <SchemaForm<T>
              schema={schema}
              project={project}
              kind={kind}
              uiSchema={arrangement.uiSchema}
              formData={formData}
              disabled={disabled}
              submitLabel={submitLabel}
              submitDisabledReason={effectiveSubmitDisabledReason}
              // The verdict's reason is already in the actions, in red: once is enough (T-5589).
              submitReasonShown={Boolean(draftKind && activeName && proposeReason && !isLax && proposeReason === effectiveSubmitDisabledReason)}
              submitting={submitting || saving}
              extraErrors={check.shownErrors}
              onSubmit={handleSubmit}
              onChange={(next) => {
                // What the browser refused is about the form as it was: a keystroke makes it stale,
                // and rjsf's own live validation takes over from here.
                check.clearSchemaErrors();
                onChange?.(next);
              }}
              afterFields={afterFields}
              actions={
                <ResourceFormFooter {...footer}>
                  {arrangement.advancedFields ? (
                    <>
                      <Checkbox
                        label={t("form.advancedMode")}
                        aria-describedby={advancedHintId}
                        checked={arrangement.advanced}
                        onChange={(event) => arrangement.setAdvanced(event.target.checked, true)}
                      />
                      <span id={advancedHintId} className="sr-only">
                        {t("form.advancedModeHint")}
                      </span>
                    </>
                  ) : null}
                </ResourceFormFooter>
              }
            />
            </Suspense>
          </div>
        ) : view === "history" && project && draftKind && lockedName ? (
          <div {...tabPanelProps(viewsId, "history")} className="flex flex-col gap-3">
            <ResourceHistory
              project={project}
              kind={draftKind}
              name={lockedName}
              onRestore={(manifest, id) => {
                if (!source) return;
                onChange?.(source.fromManifest(manifest));
                setRestored(id);
                yaml.setIssues([]);
                setView("form");
              }}
            />
          </div>
        ) : (
          <ResourceYamlPanel
            editor={yaml}
            viewsId={viewsId}
            footer={
              <ResourceFormFooter {...footer} className="justify-end" testId="form-footer">
                {effectiveSubmitDisabledReason && !proposeReason ? (
                  <span role="status" className="text-caption text-fg-muted">
                    {effectiveSubmitDisabledReason}
                  </span>
                ) : null}
                {/* `disabledReason`, as the form view's submit has it (theme.tsx): a hard-disabled
                    button leaves the tab order, and the reason written for it was then never read by
                    the person tabbing the footer (UI-44, T-1753). */}
                <Button
                  variant="primary"
                  data-shortcut="save"
                  disabled={disabled || Boolean(effectiveSubmitDisabledReason)}
                  disabledReason={effectiveSubmitDisabledReason}
                  loading={submitting || saving}
                  onClick={submitYaml}
                >
                  {submitLabel}
                </Button>
              </ResourceFormFooter>
            }
          />
        )}
      </div>
    </FormFrame>
  );
}
