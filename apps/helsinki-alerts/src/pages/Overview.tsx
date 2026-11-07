import { Card, Grid, Loading, Page, Problem, useEntities, useFunction, useMe } from "@joinedcontext/sdk";
import { StatTiles } from "../components/StatTiles";
import { ALERT } from "../alerts";
import type { Summary } from "../../functions/summary";

/** A code of the feed in words: `roadWorks` reads "Road works", `ROAD_WORK` "Road work", `traffic` "Traffic". */
export function inWords(code: string): string {
  const words = code.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/_/g, " ").trim().toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** A day as a person reads it; the text as it came when it is no date. */
export function issuedOn(value: string | undefined): string {
  const at = value ? new Date(value) : null;
  return at && !Number.isNaN(at.getTime()) ? new Intl.DateTimeFormat("en", { dateStyle: "medium" }).format(at) : (value ?? "");
}

function Counts({ label, counts }: { label: string; counts: Record<string, number> }) {
  return (
    <div>
      <h3>{label}</h3>
      <ul>
        {Object.entries(counts)
          .sort(([, a], [, b]) => b - a)
          .map(([key, count]) => (
            <li key={key}>
              {inWords(key)}: {count}
            </li>
          ))}
      </ul>
    </div>
  );
}

/** Tiles counted from the rows, and the counts the `summary` function computes on the server. */
export function Overview() {
  const { rows, loading, error } = useEntities(ALERT);
  const summary = useFunction<Summary>("summary", {});
  const me = useMe();
  return (
    <Page label="Overview">
      <Problem error={error} />
      <StatTiles rows={rows} loading={loading} tiles={[{ label: "Alerts", agg: "count" }]} />
      <Card title="Summary">
        {summary.loading && <Loading />}
        <Problem error={summary.error} onRetry={summary.reload} />
        {summary.data && (
          <>
            <p>
              {summary.data.oldestOpen
                ? `Oldest open alert: ${summary.data.oldestOpen.name ?? summary.data.oldestOpen.id}, issued ${issuedOn(summary.data.oldestOpen.dateIssued)}`
                : "No alert is open."}
            </p>
            <Grid columns={2}>
              <Counts label="By category" counts={summary.data.byCategory} />
              <Counts label="By kind" counts={summary.data.bySubCategory} />
            </Grid>
            {summary.data.ownRecords !== undefined && <p>Alerts stewards added: {summary.data.ownRecords}</p>}
          </>
        )}
      </Card>
      {me && <p className="app-muted">Signed in as {me.name ?? me.email ?? me.id}{me.roles?.length ? ` · ${me.roles.join(", ")}` : ""}</p>}
    </Page>
  );
}
