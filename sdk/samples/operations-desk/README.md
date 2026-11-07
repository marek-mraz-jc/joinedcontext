# Operations desk

**For:** staff working through a queue of alerts, incidents or requests.
**Shape:** a dense table with search and filters on the left, the chosen item beside it
(stacked under it on a phone), and bulk actions over the selected rows for the role that may act.

## What it uses
- `useEntities` and `useFilters` (a search and three selects) for the queue.
- `useAccess().can("updateAttrs", "Alert", "status")`: a person without the editor role sees the
  bulk actions disabled with the reason the endpoint gives, never a button that fails.
- `useSave().update` once per selected alert, and a summary of what went through and what did not.
- `Page`, `Header`, `Split` from the SDK; this sample's `EntityTable` with a checkbox column
  (`ColumnDef.render`), `FilterBar`, `SearchBox`, `SelectFilter`, `EntityDetail`.

## What to copy
- Severity is a word and a mark, never a colour alone (`.app-severity`).
- Sort by severity then time by default: the most urgent open item is the first row.
- The selection lives in the page, keyed by id, so filtering does not lose it, and an action says
  how many rows it touches before it runs.

## Data
`Alert` (`model.linkml.yaml`): `name`, `category`, `severity`, `status`, `district`,
`dateIssued`, `description`. The same desk works for any queue with a status to move.
