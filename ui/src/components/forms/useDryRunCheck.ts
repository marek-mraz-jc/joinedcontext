import { useEffect, useEffectEvent, useMemo, useRef, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { mergeObjects } from "@rjsf/utils";
import type { ErrorSchema } from "@rjsf/utils";
import validator from "./validator";
import { errorMessageKey } from "./errorMessages";
import { namesOfRefs } from "../../schemas/pickers";
import { api, ApiError, unwrap } from "../../api/client";
import type { ResourceProposal } from "../../api/manifest";
import type { Verdict } from "../../api/drafts";
import { atPath, controlOf, extractName, fieldPath, findingSentence, manifestOf, titleAt } from "./resourceForm";
import type { JsonSchema, ManifestSource } from "./types";

/** How long the form rests after a change before an automatic check runs. */
const AUTO_CHECK_MS = 1200;

export interface DryRunCheckOptions<T> {
  kind?: string;
  schema: JsonSchema;
  formData?: T;
  /** Whether the form view is on screen; the YAML view is checked from what `readYaml` reads. */
  formView: boolean;
  currentDigest: string;
  verdict: Verdict | null;
  updateVerdict: (verdict: Verdict | null) => void;
  canCheck: boolean;
  disabled: boolean;
  checkRefusal?: { message: string; field?: string };
  onCheck?: (data: T) => void;
  autoCheck: boolean;
  source?: ManifestSource<T>;
  project?: string;
  plural?: string;
  draftKind?: string;
  draftName?: string;
  onChange?: (data: T | undefined) => void;
  /** The form the YAML view describes, or `null` with the reason on screen. */
  readYaml: () => T | null;
  /** The YAML view was checked: what it last refused is no longer what it holds. */
  onYamlChecked: () => void;
  /** The advanced fields are folded away, and a required one among them must be shown. */
  advancedFolded: boolean;
  showAdvanced: () => void;
  /** Whether the person asked for the advanced fields: the focus reruns once they show. */
  advanced: boolean;
}

export interface DryRunCheck {
  /** The check runs on what the active view holds: the form, or the YAML read back into it. */
  runCheck: () => void;
  pending: boolean;
  /** This dialog's own check refused, in the server's words. */
  error: string | null;
  /** The last Check judged what the form holds now and was refused outright, by the page or here. */
  refusedWith: (pageError: string | null | undefined) => boolean;
  /** Why the last Check stopped before asking the server, while the form is the one it judged. */
  stoppedMessage: string | null;
  /** Whether the last check said applying this restarts the pipeline's stream (T-1056). */
  restartsStream: boolean;
  /** What the fields say: the browser's own refusals and the server's findings together. */
  shownErrors: ErrorSchema | undefined;
  /** A keystroke makes what the browser refused stale; rjsf's live validation takes over. */
  clearSchemaErrors: () => void;
  reset: () => void;
}

/**
 * What the schema already refuses, as sentences and as errors keyed by their own field.
 *
 * The check used to go straight to the server, so an untouched form was answered with the
 * server's own wording — `invalid resource envelope: missing field name`, or a regular
 * expression, or nothing at all — and no field was marked, which left a person and a screen
 * reader with no idea where (T-1491, UI-44, UI-45). The browser knows this much already.
 */
function schemaRefusals<T>(
  kind: string | undefined,
  form: T,
  schema: JsonSchema,
  t: TFunction,
): { marked: ErrorSchema; sentences: string[]; missing: boolean } | null {
  // A typed reference is judged by the name its picker shows, as the form itself judges it (T-2872).
  const { errors } = validator.validateFormData(namesOfRefs(kind, form), schema);
  if (errors.length === 0) {
    return null;
  }
  const marked: ErrorSchema = {};
  const sentences: string[] = [];
  for (const issue of errors) {
    const sentence = t(errorMessageKey(issue, schema));
    sentences.push(`${issue.property ?? ""} ${sentence}`.trim());
    atPath(marked, (issue.property ?? "").split(".")).push(sentence);
  }
  // A required field left empty is a manifest the server cannot even read: a reference with no
  // name answers "does not parse: untagged enum Ref" instead of anything a person can act on
  // (T-2634).
  return { marked, sentences, missing: errors.some((issue) => issue.name === "required") };
}

/** The first required field the schema finds empty, as its path. */
function firstMissingPath<T>(kind: string | undefined, form: T, schema: JsonSchema): string[] | null {
  const { errors } = validator.validateFormData(namesOfRefs(kind, form), schema);
  const issue = errors.find((candidate) => candidate.name === "required");
  return issue ? (issue.property ?? "").split(".").filter(Boolean) : null;
}

/**
 * A finding the server made, on the field it names (`fieldPath`; UI-45, T-1491). A finding with
 * no path stays in the list under the verdict, where it was.
 */
export function findingErrorsOf(
  verdict: Verdict | null,
  schema: JsonSchema,
  sentence: (message: string) => string,
): ErrorSchema | undefined {
  const located = (verdict?.findings ?? []).filter((finding) => finding.level === "error" && Boolean(finding.path));
  if (located.length === 0) {
    return undefined;
  }
  const marked: ErrorSchema = {};
  for (const finding of located) {
    atPath(marked, fieldPath(finding.path, schema)).push(sentence(finding.message));
  }
  return marked;
}

/** The message a refused check carries: the problem's own sentence for an API answer. */
function checkErrorOf(error: Error | null): string | null {
  if (!error) return null;
  return error instanceof ApiError ? (error.problem?.detail ?? error.message) : error.message;
}

/**
 * The check a dialog runs for itself: the same dry run the pages with their own `onCheck`
 * send, on the manifest the form holds and the draft the proposal will name (T-0779).
 */
function useOwnCheck<T>(options: DryRunCheckOptions<T>, setRestartsStream: (restarts: boolean) => void) {
  const { updateVerdict } = options;
  return useMutation({
    mutationFn: async (form: T) => {
      const name = options.draftName || extractName(form);
      // What a page's `toManifest` builds is a manifest the API checks; its type is the API's.
      const proposed = manifestOf(form, options.source) as ResourceProposal;
      const body: ResourceProposal =
        options.draftKind && name ? { ...proposed, draft: { kind: options.draftKind, name } } : proposed;
      return unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: {
            path: { project: options.project ?? "", plural: options.plural ?? "" },
            query: { dryRun: "All" },
          },
          body: body,
        }),
      );
    },
    onSuccess: (result) => {
      const answer = result as { verdict?: Verdict; restartsStream?: boolean };
      updateVerdict(answer?.verdict ?? null);
      // T-1056, PL-45: the runner restarts a stream whenever its render moves, and a periodic
      // pipeline then starts its schedule over. A person reads that before proposing, not after.
      setRestartsStream(answer?.restartsStream === true);
    },
    // A refused check judged nothing: the chip says "not checked", never "changed since".
    onError: () => {
      updateVerdict(null);
    },
  });
}

/**
 * The server's findings used to be dropped whenever the browser marked any field at all, so a
 * name another project holds went unsaid on the name while the space picker was marked (T-2731,
 * UI-45); a field both judge shows both sentences.
 */
function useShownErrors(
  verdict: Verdict | null,
  schema: JsonSchema,
  schemaErrors: ErrorSchema | undefined,
): ErrorSchema | undefined {
  const { t } = useTranslation();
  const findingErrors = useMemo(
    () => findingErrorsOf(verdict, schema, (message) => findingSentence(message, t)),
    [verdict, schema, t],
  );
  return useMemo<ErrorSchema | undefined>(() => {
    if (schemaErrors === undefined || findingErrors === undefined) {
      return schemaErrors ?? findingErrors;
    }
    return mergeObjects(findingErrors, schemaErrors, "preventDuplicates") as ErrorSchema;
  }, [schemaErrors, findingErrors]);
}

// The automatic check reads what the latest render holds; it is keyed on the form alone, so a
// verdict arriving re-renders without sending the same form again.
function useAutoCheck<T>(options: DryRunCheckOptions<T>, check: (form: T) => void) {
  const { t } = useTranslation();
  const { autoCheck, formView, formData } = options;
  const autoChecked = useRef<T | undefined>(undefined);
  const autoRun = useEffectEvent((form: T) => {
    const { kind, schema } = options;
    if (!options.canCheck || options.disabled || options.checkRefusal || schemaRefusals(kind, form, schema, t)?.missing) {
      return;
    }
    autoChecked.current = form;
    check(form);
  });
  useEffect(() => {
    if (!autoCheck || !formView || formData === undefined || autoChecked.current === formData) {
      return;
    }
    const timer = setTimeout(() => autoRun(formData), AUTO_CHECK_MS);
    return () => clearTimeout(timer);
  }, [autoCheck, formView, formData]);
}

// The field a stopped check names is opened, scrolled to and focused once it is on screen,
// with every fold around it opened (T-3970); `advanced` reruns it once those fields show.
function useFocusStopped(stopped: { field?: string } | null, formView: boolean, advanced: boolean) {
  useEffect(() => {
    const target = formView && stopped?.field ? controlOf(stopped.field) : null;
    if (!target) {
      return;
    }
    for (let fold = target.closest("details"); fold; fold = fold.parentElement?.closest("details") ?? null) {
      fold.open = true;
    }
    target.scrollIntoView?.({ block: "center" });
    target.focus();
  }, [stopped, formView, advanced]);
}

/**
 * The Check of a form (PF-57): the browser's schema first, the server's dry run after it, the
 * findings on the fields they name and the first empty required field brought into view.
 */
export function useDryRunCheck<T>(options: DryRunCheckOptions<T>): DryRunCheck {
  const { kind, schema, formData, formView, currentDigest, canCheck, checkRefusal } = options;
  const { t } = useTranslation();
  /** What the browser's own check refused, keyed by field, until the form changes (T-1491). */
  const [schemaErrors, setSchemaErrors] = useState<ErrorSchema | undefined>(undefined);
  // Why the last Check stopped before asking the server, and the field to bring into view: a
  // count of filled fields beside the button left a person searching a long form (T-3970).
  // It holds the form it judged, so any change to the draft, from a field or from a workbench
  // beside the form, retires it.
  const [stopped, setStopped] = useState<{ message: string; field?: string; form?: T } | null>(null);
  const [restartsStream, setRestartsStream] = useState(false);
  /**
   * What the form held when Check was last pressed: a check the server refused outright (an
   * invalid manifest answers with a problem, not a verdict) leaves no verdict, and the chip says
   * the check failed rather than "not checked yet" until the form changes (T-4824).
   */
  const [checkedDigest, setCheckedDigest] = useState<string | null>(null);

  const ownCheck = useOwnCheck(options, setRestartsStream);
  const check = options.onCheck ?? ((form: T) => ownCheck.mutate(form));
  const shownErrors = useShownErrors(options.verdict, schema, schemaErrors);

  function checkForm(form: T) {
    // What the browser already knows goes onto the fields at once, in words (T-1491); the
    // server is still asked, because its check sees what the schema cannot — a reference that
    // does not resolve, a name already taken, a grant the proposer does not have. Not while a
    // required field is empty: that manifest does not parse, and the field already says what
    // it needs (T-2634).
    const refused = schemaRefusals(kind, form, schema, t);
    setSchemaErrors(refused?.marked);
    if (!refused?.missing) {
      check(form);
      return;
    }
    const path = firstMissingPath(kind, form, schema);
    const field = path?.join(".");
    setStopped(path ? { message: t("form.missing", { field: titleAt(schema, path) }), field, form } : null);
    // A required field among the advanced ones is not on screen until they are shown.
    if (field !== undefined && !controlOf(field) && options.advancedFolded) {
      options.showAdvanced();
    }
  }

  function checkYaml() {
    const form = options.readYaml();
    if (form === null) return;
    // The YAML view is checked by the server alone, on purpose: it is where a value the form's
    // own schema cannot express is typed — a rate limit outside the offered classes, a field the
    // kind has no control for — and the parent widens the schema only one render later (T-0890).
    // Refusing it here would refuse what the REST route accepts.
    options.onChange?.(form);
    setSchemaErrors(undefined);
    options.onYamlChecked();
    check(form);
  }

  function runCheck() {
    if (!canCheck) {
      return;
    }
    setStopped(null);
    setCheckedDigest(currentDigest);
    if (checkRefusal) {
      setStopped({ ...checkRefusal, form: formData });
    } else if (!formView) {
      checkYaml();
    } else if (formData) {
      checkForm(formData);
    }
  }

  useAutoCheck(options, check);
  useFocusStopped(stopped, formView, options.advanced);

  return {
    runCheck,
    pending: ownCheck.isPending,
    error: checkErrorOf(ownCheck.error),
    refusedWith: (pageError) => checkedDigest === currentDigest && Boolean(pageError ?? ownCheck.error),
    stoppedMessage: stopped && stopped.form === formData ? stopped.message : null,
    restartsStream,
    shownErrors,
    clearSchemaErrors: () => setSchemaErrors(undefined),
    reset: () => setRestartsStream(false),
  };
}
