/**
 * The SDK's one shell (SDK-39): every App looks and behaves alike, and the shell holds the entity
 * panel a map, a table or a chart opens (SDK-40). `navigate` opens a page by its id through the
 * address, which the shell follows.
 */
export { AppShell } from "@joinedcontext/sdk";
export type { ShellPage as Page } from "@joinedcontext/sdk";

export function navigate(id: string): void {
  if (typeof window !== "undefined") window.location.hash = `#/${id}`;
}
