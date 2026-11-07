# Form-first workflow

**For:** a resident reporting a problem, an employee filing a request, anyone sending one thing
and coming back to see what happened to it.
**Shape:** a form in four steps (what, where, contact, check and send) with the person's own
earlier submissions beside it (under it on a phone).

## What it uses
- `useSchema("ServiceRequest")`: the endpoint's JSON Schema, which Model Tools generates from
  `model.linkml.yaml`. `fieldOf` turns each slot into an input (an `enum` becomes a select, a
  `pattern` a checked text), and the template's `parseInput` checks an answer against it. The
  form waits for the schema: without the rules there is no form.
- `useSave().create` for the new entity; `useAccess().can("createEntity", …)` disables Send with
  the endpoint's reason instead of letting the write fail.
- `useMe()` for who is sending, and `useEntities` with `q=submittedBy=="<id>"` for their list.
- `Page`, `Header`, `Split`, `Card` from the SDK.

## What to copy
- Each step is checked on Next; the problems appear in a summary that takes focus and links to
  each input, and each input carries `aria-invalid` and its message by `aria-describedby`.
- The rule is shown before it is broken: a hint under each input with a limit.
- The last step shows every answer with a Change button back to its step. Send checks all steps
  again and returns to the first one with a problem.
- After sending, the person gets a reference and the request appears in their list.

## Security
- `submittedBy` is what the browser says; a check that must hold (who sent what) belongs on the
  server, for example a function that stamps it from the token.
- The "Your requests" list is a query, not a control: when requests must stay private between
  residents, the endpoint has to refuse reading other people's requests.
- The id only goes into the query when it looks like an account id (`ownerQuery`); anything else
  is not listed.

## Data
`ServiceRequest` (`model.linkml.yaml`): `category`, `title`, `description`, `address`,
`district`, `contactEmail`, `mayContact`, `submittedBy`, `dateSubmitted`, `status`.
