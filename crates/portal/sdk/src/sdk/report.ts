/**
 * Logs an error and, inside the preview frame, posts it to the Portal as `jc-error` with the file
 * and line of its first stack frame, so the run can hand it back to the model (SDK-14). Never
 * throws.
 */
export function reportError(error: unknown): void {
  try {
    console.error("jc:", error);
    if (typeof window !== "undefined" && window.parent && window.parent !== window) {
      const message = error instanceof Error ? error.message : String(error);
      let file: string | undefined;
      let line: number | undefined;

      if (error instanceof Error && typeof error.stack === "string") {
        const lines = error.stack.split("\n");
        for (const l of lines) {
          const match = l.match(/(\S+?):(\d+):\d+\)?$/);
          if (match) {
            file = match[1];
            line = Number(match[2]);
            break;
          }
        }
      }

      window.parent.postMessage({ kind: "jc-error", message, file, line }, "*");
    }
  } catch {
    // wrapped in try/catch, never throws
  }
}
