import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Tabs, tabPanelProps } from "../../components/ui";
import { LinkmlPreviewPanel } from "./LinkmlPreviewPanel";
import { LinkmlGraphView } from "./LinkmlGraphView";
import { LinkmlSourceEditor } from "./LinkmlSourceEditor";
import { LinkmlVisualEditor } from "./LinkmlVisualEditor";
import { ModelSubsetPicker } from "./ModelSubsetPicker";
import { diagnose, parseModel } from "./linkml";
import { EMPTY_SUBSET, subsetSource } from "./subset";
import type { Subset } from "./subset";

/**
 * The one LinkML editor every page embeds (DM-13, DM-17).
 *
 * It is controlled: the document is the `source` string the caller holds, every view edits
 * that string through `onChange`, and nothing here has a second copy. That is also what makes
 * it drivable from outside — an assistant applies `applyOperations` to the same string and the
 * tree shows the result (DM-31), a test sets the string and reads the tree.
 *
 * Two modes, one component. With `onSubsetChange` the editor is a picker: the model as a tree of
 * checkboxes and the preview of the narrowed model an endpoint would serve (MP-01, MP-03).
 * Without it, the editor edits: structure, source and preview of the whole model.
 */
export type EditorView = "structure" | "source" | "graph" | "preview" | "subset";

export interface LinkmlEditorProps {
  source: string;
  onChange: (source: string) => void;
  /** The organisation's locales, for the language maps every title needs (DM-15). */
  locales?: string[];
  /** Subset mode: the classes and slots ticked so far, and where a tick goes. */
  subset?: Subset;
  onSubsetChange?: (subset: Subset) => void;
  initialView?: EditorView;
  /** Where the empty model's "Import a Smart Data Model" hint goes, when the page offers it. */
  onImport?: (from: "sdm" | "file") => void;
  /** The drawing without edit handles, for a person who may not propose the model (T-3589). */
  readOnlyDrawing?: boolean;
}

// The order the owner reads a model in (T-6101): what it holds, how it connects, what an entity
// looks like, and the LinkML last.
const EDIT_VIEWS: EditorView[] = ["structure", "graph", "preview", "source"];
const SUBSET_VIEWS: EditorView[] = ["subset", "preview"];

export function LinkmlEditor({
  source,
  onChange,
  locales = [],
  subset,
  onSubsetChange,
  initialView,
  onImport,
  readOnlyDrawing = false,
}: LinkmlEditorProps): JSX.Element {
  const { t } = useTranslation();
  const picking = onSubsetChange !== undefined;
  const views = picking ? SUBSET_VIEWS : EDIT_VIEWS;
  const [view, setView] = useState<EditorView>(initialView ?? views[0]);
  // The class a click in the diagram opened, so the structure view opens on that one.
  const [opened, setOpened] = useState<string | undefined>(undefined);

  const model = useMemo(() => parseModel(source), [source]);
  const openClass = (name: string) => {
    setOpened(name);
    setView("structure");
  };
  const diagnostics = useMemo(() => diagnose(source, locales), [source, locales]);
  const chosen = subset ?? EMPTY_SUBSET;
  const previewed = useMemo(
    () => (picking ? subsetSource(source, chosen) : source),
    [picking, source, chosen],
  );

  return (
    <div className="flex flex-col gap-3">
      <Tabs
        id="linkml-view"
        variant="pill"
        label={t("models.title")}
        tabs={views.map((name) => ({ value: name, label: t(`models.view.${name}`) }))}
        value={view}
        onChange={setView}
      />
      <div {...tabPanelProps("linkml-view", view)}>
        {view === "structure" ? (
          <LinkmlVisualEditor
            key={opened}
            initialClass={opened}
            source={source}
            onChange={onChange}
            diagnostics={diagnostics}
            locales={locales}
            onImport={onImport}
          />
        ) : null}
        {view === "graph" ? (
          // A class opens in the structure view. A relationship is added, changed and removed on
          // the drawing itself (T-3589); read-only, its line and "+" open the class instead.
          <LinkmlGraphView
            source={source}
            onOpenClass={openClass}
            onOpenRelationship={openClass}
            onAddRelationship={openClass}
            onChange={readOnlyDrawing ? undefined : onChange}
          />
        ) : null}
        {view === "source" ? (
          <LinkmlSourceEditor
            source={source}
            onChange={onChange}
            diagnostics={diagnostics}
            readOnly={readOnlyDrawing}
          />
        ) : null}
        {view === "subset" && onSubsetChange ? (
          <ModelSubsetPicker model={model} subset={chosen} onChange={onSubsetChange} />
        ) : null}
        {view === "preview" ? <LinkmlPreviewPanel source={previewed} /> : null}
      </div>
    </div>
  );
}
