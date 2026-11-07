# Services desk

**For:** staff working through defect reports who keep photos with a report, want a first
rating, mail a report to themselves and look after the App's scheduled jobs.
**Shape:** the reports on the left, the chosen one beside them (under them on a phone) with its
photos and three actions, and the App's jobs below.

## What it uses
- `useEntities("DefectReport")` for the reports.
- `useClient().files.list` and `.files.put` under `reports/{id}/`: the folder comes from the
  report's id and the key from the file name, both reduced to letters, digits, `.`, `-`, `_`.
- `useClient().ai.complete` with a JSON Schema, so the answer is a number between 1 and 5 or no
  rating at all.
- `useClient().email.send({ to: "me", … })`: the Portal sends to the signed-in person's address;
  the page never holds an address.
- `useClient().jobs.list()` for each job's schedule, next run and last run.
- `useClient().functions.call("close-report", …)`, and in `functions/close-report.ts`
  `requireRole(request, "steward")` before anything is read.
- `Page`, `Header`, `Split`, `Card` from the SDK.

## What to copy
- `refusal()` in `src/refusal.ts`: a switched-off service names the layer that switched it off
  (organization, project or App), a used-up quota names the quota and when it comes back, a role
  refusal says which role. Each shows as an alert beside the action that was refused.
- Each action keeps its own answer and is disabled while it runs, so a double click sends once.

## Security
- The steward check is in the function, on the person the Portal verified; the button is shown
  to everyone and a clerk's click is refused by the function.
- The services' switches and quotas are the Portal's: the page shows a refusal, it never decides
  one.

## Data
`DefectReport` (`model.linkml.yaml`): `name`, `description`, `district`, `status`,
`dateReported`. The function writes `status` only.
