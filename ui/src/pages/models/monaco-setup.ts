/// <reference types="vite/client" />
/**
 * Monaco, bundled with the Portal rather than fetched from a CDN.
 *
 * `@monaco-editor/react` loads the editor from jsDelivr by default. A municipal portal runs
 * behind its own Content Security Policy and, on some installations, with no route to the
 * public internet at all, so the loader is pointed at the copy in this bundle and the worker
 * is the one Vite emits.
 *
 * The bare `monaco-editor` import registers about 80 languages and the TypeScript, CSS, HTML and
 * JSON services with their workers (8.8 MB of the build, T-5513), and `features/register.all` adds
 * the diff editor, inline completions, code lens and 50 more the YAML views never open. This takes
 * the editor API, the YAML grammar and the features the source views use: editing, hover
 * (T-5052 marks), folding, find, completion (LinkmlSourceEditor), marker navigation (F8), the
 * read-only notice, Ctrl+M to tab out of the editor, and the invisible-character highlight.
 */
import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api";
import "monaco-editor/features/codeEditor/register";
import "monaco-editor/editor/browser/coreCommands";
import "monaco-editor/features/bracketMatching/register";
import "monaco-editor/features/clipboard/register";
import "monaco-editor/features/comment/register";
import "monaco-editor/features/contextmenu/register";
import "monaco-editor/features/cursorUndo/register";
import "monaco-editor/features/find/register";
import "monaco-editor/features/folding/register";
import "monaco-editor/features/gotoError/register";
import "monaco-editor/features/hover/register";
import "monaco-editor/features/indentation/register";
import "monaco-editor/features/linesOperations/register";
import "monaco-editor/features/readOnlyMessage/register";
import "monaco-editor/features/smartSelect/register";
import "monaco-editor/editor/contrib/suggest/browser/suggestController";
import "monaco-editor/features/toggleTabFocusMode/register";
import "monaco-editor/features/tokenization/register";
import "monaco-editor/features/unicodeHighlighter/register";
import "monaco-editor/features/wordHighlighter/register";
import "monaco-editor/features/wordOperations/register";
import "monaco-editor/languages/definitions/yaml/register";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";

(self as unknown as { MonacoEnvironment?: { getWorker: () => Worker } }).MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

loader.config({ monaco });
