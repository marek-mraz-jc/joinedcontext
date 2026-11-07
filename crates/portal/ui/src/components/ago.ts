/** "12 s ago", "4 min ago", "3 h ago", "2 d ago" from an RFC 3339 instant; the raw text when it is not one. */
export function ago(iso: string, now: number, t: (key: string, values?: Record<string, unknown>) => string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) {
    return iso;
  }
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 60) {
    return t("agentRun.catalog.agoSeconds", { n: seconds });
  }
  if (seconds < 3600) {
    return t("agentRun.catalog.agoMinutes", { n: Math.round(seconds / 60) });
  }
  if (seconds < 86400) {
    return t("agentRun.catalog.agoHours", { n: Math.round(seconds / 3600) });
  }
  return t("agentRun.catalog.agoDays", { n: Math.round(seconds / 86400) });
}
