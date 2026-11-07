# helsinki-alerts

Road works and traffic announcements in the capital region, read from the `helsinki` context
space: an overview with the counts the `summary` function computes, and an Alerts page with
filters, a map, a table and the detail of one alert. Sample application 3 of 3 (T-2598): a React
frontend on the App SDK plus serverless functions, with two roles.

| Role | Who (dev) | May |
|---|---|---|
| `viewer` | `demo.viewer@hel.fi` | read every alert and the summary |
| `steward` | `demo.steward@hel.fi` | also add alerts, correct their fields, and remove the alerts a steward added |

The roles are enforced by the gateway, not by this page (AP-96): `app.yaml` grants the write items
to `steward` only, and the delete item carries `q: "!source"`, so only a record with no `source`
(one a steward added, never Fintraffic's) can be removed. An alert on the map or in the table
opens in the SDK shell's entity panel (SDK-40). There a steward corrects the plain fields the
`steward` write item grants (`category`, `subCategory`, `address`, `dateIssued`, `validFrom`,
`validTo`): the change is shown first and saved as one PATCH, and the table reads the alerts again.
A viewer gets no Edit, only the link to the alert in the Portal. The App's own form, from the
toolbar above the map, adds an alert and corrects what the panel leaves to the Portal: `name` and
`description` language by language, keeping the others, and the `location`. It writes every
attribute of `Alert` but `source` and lists the fields it leaves alone and why. The data needs are
unchanged: the write items were the steward's before the panel came. A later run of the traffic-messages pipeline overwrites a correction of
a Fintraffic record's fields.

## Functions

- `functions/summary.ts`: counts per `category` and `subCategory`, the oldest open alert, and for
  a steward the number of alerts stewards added (`request.user.roles`).
- `functions/expiring.ts`: the alerts whose `validTo` falls in the next `hours` (1 to 168).

Both read only through `ctx.jc` with the caller's own grants and hold no secret.

## Run it

```sh
pnpm install
pnpm test                 # the pages and both functions
pnpm build && pnpm e2e    # the built bundle in Chromium against the SDK's stub
```

Against dev, `pnpm dev` with `JC_ENDPOINT_SLUG` set to the app's endpoint serves the pages with
your own sign-in. The repository of record is
`https://dev.joinedcontext.com/git/joinedcontext/helsinki_helsinki-alerts.git`; the build lane
builds `main` with `.gitea/workflows/build.yml` (AP-75, AP-80).
