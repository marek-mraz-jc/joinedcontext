/**
 * Every shared control, in every state, on one route (T-1729; UI-15, UI-16).
 *
 * A component's states — disabled, busy, refused, empty, a label longer than the box, a label of
 * three characters — are where the Portal's controls break, and until this page existed they were
 * only ever seen inside whichever page happened to use one. Here they are all at once: `axe` runs
 * over the same list in `gallery_axe.test.tsx`, and Playwright takes the picture of it at the
 * recording's size and at phone width, so a change to a token, a radius or a focus ring is a diff
 * a person reviews rather than a surprise on a page nobody opened.
 *
 * Development only. The route is added in `router.tsx` under `import.meta.env.DEV`, and nothing
 * else imports this module, so a production build drops it; `gallery_axe.test.tsx` holds that.
 */
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  CardHeader,
  Checkbox,
  CodeBlock,
  RecordView,
  ConfirmDialog,
  Dialog,
  DialogClose,
  EmptyState,
  ExternalLink,
  Field,
  FileDropZone,
  InlineError,
  FilePicker,
  Icon,
  Input,
  Menu,
  MenuContent,
  MenuItem,
  MenuLabel,
  MenuSeparator,
  MenuTrigger,
  PageFailed,
  PageHeader,
  PageLoading,
  RadioGroup,
  RowActions,
  Select,
  Skeleton,
  SourceLink,
  Switch,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableRowHeaderCell,
  TableSkeleton,
  tabPanelProps,
  Tabs,
  Term,
  Textarea,
} from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { ManifestGrid } from "../../components/grid/ManifestGrid";

const GRID_ROWS = [
  { name: "air-quality", audience: "public", calls: 1204, space: "air" },
  { name: "parking-occupancy", audience: "organization", calls: 87, space: "mobility" },
  { name: "street-works", audience: "public", calls: 0, space: "works" },
];

/**
 * A label of sixty characters, in German, where the words are longest. Every control is shown
 * with it: what wraps, what clips and what pushes its row out of the page shows up here first.
 *
 * It is a phrase, not one sixty-character compound. A single unbreakable word of that length is
 * not a label anybody writes, and holding the shared controls to it only moved where their text
 * wraps on real pages (T-2413: it moved the `spaces` baseline at 400 px and nothing else).
 * Where an unbreakable string is real — an entity id — it is `URN` below, in the places that
 * actually carry one.
 */
const LONG = "Luftqualitätsmessstation für die Verwaltung der Berechtigungen";

/** What an unbreakable string really looks like in the Portal: an entity id (EP-08). */
const URN = "urn:ngsi-ld:AirQualityObserved:banskabystrica:ovzdusie:stanica-01";

/** And the other end: three characters, where a control collapses to its padding. */
const SHORT = "Ja!";

function Specimen({ name, children }: { name: string; children: ReactNode }): JSX.Element {
  return (
    <section className="flex flex-col gap-2" aria-labelledby={`gallery-${name}`}>
      <h2 id={`gallery-${name}`} className="text-title font-semibold text-fg">
        {name}
      </h2>
      <div className="flex flex-wrap items-start gap-3 rounded-lg border border-border bg-surface p-4">
        {children}
      </div>
    </section>
  );
}

/** One state of one control, with the words that say which state it is. */
function State({ is, children }: { is: string; children: ReactNode }): JSX.Element {
  return (
    <div className="flex min-w-0 flex-col gap-1 sm:min-w-40">
      <span className="text-caption text-fg-muted">{is}</span>
      {children}
    </div>
  );
}

export function Gallery(): JSX.Element {
  const [checked, setChecked] = useState(true);
  const [choice, setChoice] = useState<"one" | "two">("one");
  const [tab, setTab] = useState<"first" | "second">("first");
  const [dialog, setDialog] = useState(false);
  const [confirm, setConfirm] = useState(false);

  return (
    <main className="mx-auto flex max-w-5xl flex-col gap-section p-6">
      <PageHeader
        title="Component gallery"
        description="Every shared control, every state. Development only (T-1729)."
        actions={<Button variant="primary">{SHORT}</Button>}
      />

      <Specimen name="Button">
        {(["primary", "secondary", "ghost", "danger"] as const).map((variant) =>
          (["xs", "sm", "md", "lg"] as const).map((size) => (
            <State key={`${variant}-${size}`} is={`${variant} ${size}`}>
              <Button variant={variant} size={size}>
                Propose the change
              </Button>
            </State>
          )),
        )}
        <State is="with an icon">
          <Button icon={<Icon name="refresh" className="size-4" />}>Read it again</Button>
        </State>
        <State is="busy">
          <Button loading>Proposing…</Button>
        </State>
        <State is="disabled">
          <Button disabled>Propose the change</Button>
        </State>
        <State is="refused, with the reason">
          {/* Both: `Button` explains itself only when it is disabled *and* carries a reason. */}
          <Button disabled disabledReason="Your role does not permit 'propose' on 'Endpoint'">
            Propose the change
          </Button>
        </State>
        <State is="a label of sixty characters">
          <Button>{LONG}</Button>
        </State>
        <State is="a label of three">
          <Button>{SHORT}</Button>
        </State>
        <State is="icon only, named">
          <Button
            aria-label="Read it again"
            title="Read it again"
            icon={<Icon name="refresh" className="size-4" />}
          />
        </State>
      </Specimen>

      <Specimen name="Alert">
        {(["info", "success", "warning", "danger"] as const).map((tone) => (
          <State key={tone} is={tone}>
            <Alert tone={tone} title={tone === "danger" ? "The change was refused" : undefined}>
              {LONG}
            </Alert>
          </State>
        ))}
        <State is="with an action">
          <Alert tone="danger" actions={<Button size="sm">Try again</Button>}>
            The store did not answer.
          </Alert>
        </State>
      </Specimen>

      <Specimen name="InlineError">
        <State is="short">
          <InlineError>The list could not be read.</InlineError>
        </State>
        <State is="long">
          <InlineError>{LONG}</InlineError>
        </State>
        <State is="with a retry">
          <InlineError>
            Data sources: the list could not be read.{" "}
            <Button size="xs" variant="ghost">
              Try again
            </Button>
          </InlineError>
        </State>
      </Specimen>

      <Specimen name="Badge">
        {(["neutral", "primary", "success", "warning", "danger", "info", "accent"] as const).map((tone) => (
          <State key={tone} is={tone}>
            <Badge tone={tone}>{tone}</Badge>
          </State>
        ))}
        <State is="mono, an entity id">
          <Badge mono>{URN}</Badge>
        </State>
      </Specimen>

      <Specimen name="Card, CardHeader">
        <State is="with a header and a description">
          <Card className="w-full max-w-80">
            <CardHeader title={LONG} description="What this card is for." actions={<Badge>2</Badge>} />
          </Card>
        </State>
        <State is="flush, for a table or a map">
          <Card flush className="w-full max-w-40">
            <Skeleton className="h-16 w-full" />
          </Card>
        </State>
      </Specimen>

      <Specimen name="Field, Input, Textarea, Select">
        <State is="required, with help">
          <Field id="gallery-name" label="Name" required description="Lower case, dashes." help="air-quality">
            <Input id="gallery-name" defaultValue="air-quality" />
          </Field>
        </State>
        <State is="refused by the server">
          <Field id="gallery-bad" label={LONG} errors={["A name may hold letters, digits and dashes."]}>
            <Input id="gallery-bad" defaultValue="Ovzduší!" aria-invalid />
          </Field>
        </State>
        <State is="disabled">
          <Field id="gallery-off" label={SHORT}>
            <Input id="gallery-off" disabled defaultValue="steward" />
          </Field>
        </State>
        <State is="a paragraph">
          <Field id="gallery-text" label="Description">
            <Textarea id="gallery-text" rows={3} defaultValue={LONG} />
          </Field>
        </State>
        <State is="a choice">
          <Field id="gallery-select" label="Phase">
            <Select id="gallery-select" defaultValue="live">
              <option value="live">Live</option>
              <option value="pending">Pending</option>
            </Select>
          </Field>
        </State>
      </Specimen>

      <Specimen name="Checkbox, Switch, RadioGroup">
        <State is="ticked, with a hint">
          <Checkbox
            label="Publish this endpoint"
            hint="Anyone with the address may read it."
            checked={checked}
            onChange={(event) => setChecked(event.currentTarget.checked)}
          />
        </State>
        <State is="refused, with the reason">
          <Checkbox label={SHORT} checked={false} readOnly disabled disabledReason="Your role does not permit 'propose'" />
        </State>
        <State is="on">
          <Switch checked={checked} onCheckedChange={setChecked} label={LONG} />
        </State>
        <State is="a row of options">
          <RadioGroup
            name="gallery-radio"
            legend="What the copy holds"
            value={choice}
            onChange={setChoice}
            options={[
              { value: "one", label: SHORT, description: "The short one." },
              { value: "two", label: LONG, disabled: true },
            ]}
          />
        </State>
      </Specimen>

      <Specimen name="Tabs">
        <State is="line, two tabs">
          <Tabs
            id="gallery-tabs"
            label="What the gallery shows"
            value={tab}
            onChange={setTab}
            tabs={[
              { value: "first", label: SHORT },
              { value: "second", label: LONG },
            ]}
          />
          {/* The panel belongs to the page, not to `Tabs`: without it the `aria-controls` of
              each tab points at nothing. */}
          <div {...tabPanelProps("gallery-tabs", tab)} className="p-2 text-body">
            {tab === "first" ? SHORT : LONG}
          </div>
        </State>
        <State is="pill">
          <Tabs
            id="gallery-tabs-pill"
            label="The same, as pills"
            variant="pill"
            value={tab}
            onChange={setTab}
            tabs={[
              { value: "first", label: "First" },
              { value: "second", label: "Second" },
            ]}
          />
          <div {...tabPanelProps("gallery-tabs-pill", tab)} className="p-2 text-body">
            {tab === "first" ? "First" : "Second"}
          </div>
        </State>
      </Specimen>

      <Specimen name="Table">
        <State is="two rows">
          <Table caption="What the project holds">
            <TableHead>
              <TableHeaderCell>Name</TableHeaderCell>
              <TableHeaderCell align="right">State</TableHeaderCell>
            </TableHead>
            <TableBody>
              <TableRow>
                <TableRowHeaderCell>{URN}</TableRowHeaderCell>
                <TableCell align="right">
                  <Badge tone="success">Live</Badge>
                </TableCell>
              </TableRow>
              <TableRow>
                <TableRowHeaderCell>{SHORT}</TableRowHeaderCell>
                <TableCell align="right">
                  <RowActions
                    label={SHORT}
                    primary={<Button size="sm">Open</Button>}
                    actions={[
                      { key: "edit", label: "Edit", onSelect: () => undefined },
                      {
                        key: "remove",
                        label: "Remove",
                        tone: "danger",
                        onSelect: () => undefined,
                        disabledReason: "Your role does not permit 'delete'",
                      },
                    ]}
                  />
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </State>
        <State is="waiting">
          <Table caption="Reading the endpoints" status="Loading">
            <TableHead>
              <TableHeaderCell>Name</TableHeaderCell>
              <TableHeaderCell>State</TableHeaderCell>
            </TableHead>
            <TableSkeleton columns={2} />
          </Table>
        </State>
        <State is="empty">
          <Table caption="Nothing of this kind">
            <TableHead>
              <TableHeaderCell>Name</TableHeaderCell>
            </TableHead>
            <TableBody>
              <TableEmpty columns={1}>
                <EmptyState bare title="Nothing here yet." description="Add the first one." />
              </TableEmpty>
            </TableBody>
          </Table>
        </State>
      </Specimen>

      <Specimen name="ManifestGrid">
        <State is="three rows, a filter and hidden columns">
          <ManifestGrid
            caption="Endpoints of the project"
            storageKey="gallery"
            searchLabel="Search endpoints"
            rows={GRID_ROWS}
            rowKey={(row) => row.name}
            query={{ isPending: false, isError: false, error: null, refetch: () => undefined }}
            name={{ id: "name", label: "Name", text: (row) => row.name }}
            columns={[
              { id: "audience", label: "Audience", filter: true, text: (row) => row.audience },
              { id: "calls", label: "Calls in 7 days", numeric: true, align: "right", text: (row) => String(row.calls) },
              { id: "space", label: "Context Space", hidden: true, text: (row) => row.space },
            ]}
            empty={<EmptyState bare title="No endpoints yet" />}
          />
        </State>
        <State is="waiting">
          <ManifestGrid
            caption="Reading the endpoints"
            storageKey="gallery-waiting"
            searchLabel="Search endpoints"
            rows={[] as typeof GRID_ROWS}
            rowKey={(row) => row.name}
            query={{ isPending: true, isError: false, error: null, refetch: () => undefined }}
            name={{ id: "name", label: "Name", text: (row) => row.name }}
            columns={[{ id: "audience", label: "Audience", text: (row) => row.audience }]}
            empty={<EmptyState bare title="No endpoints yet" />}
          />
        </State>
      </Specimen>

      <Specimen name="EmptyState">
        <State is="with an action">
          <EmptyState
            icon="approvals"
            title="Nothing is waiting for you"
            description={LONG}
            action={<Button variant="primary">Ask the assistant</Button>}
          />
        </State>
        <State is="bare">
          <EmptyState bare title={SHORT} />
        </State>
      </Specimen>

      <Specimen name="PageLoading, PageFailed, Skeleton">
        <State is="waiting">
          <PageLoading label="Reading the endpoints" lines={2} />
        </State>
        <State is="refused, with what the API said">
          <PageFailed error={new Error("gone")} onRetry={() => undefined} />
        </State>
        <State is="one bar">
          <Skeleton className="h-4 w-40" />
        </State>
      </Specimen>

      <Specimen name="Menu, Dialog, ConfirmDialog">
        <State is="a menu">
          <Menu>
            <MenuTrigger asChild>
              <Button size="sm" variant="secondary" aria-label="More actions for the gallery">
                ⋯
              </Button>
            </MenuTrigger>
            <MenuContent align="end">
              <MenuLabel>{SHORT}</MenuLabel>
              <MenuItem>Edit</MenuItem>
              <MenuSeparator />
              <MenuItem tone="danger">Remove</MenuItem>
            </MenuContent>
          </Menu>
        </State>
        <State is="a dialog, closed until it is opened">
          <Button onClick={() => setDialog(true)}>Open the dialog</Button>
          <Dialog
            open={dialog}
            onOpenChange={setDialog}
            title={LONG}
            description="What this dialog asks for."
            closeLabel="Close"
            footer={
              <>
                <DialogClose asChild>
                  <Button>Cancel</Button>
                </DialogClose>
                <Button variant="primary">Propose the change</Button>
              </>
            }
          >
            <Field id="gallery-dialog-field" label="Name">
              <Input id="gallery-dialog-field" defaultValue="air-quality" />
            </Field>
          </Dialog>
        </State>
        <State is="a confirmation, for what cannot be undone">
          <Button variant="danger" onClick={() => setConfirm(true)}>
            Remove the endpoint
          </Button>
          <ConfirmDialog
            open={confirm}
            onOpenChange={setConfirm}
            title="Remove air-quality?"
            description="Removing proposes a change; an approver confirms it."
            confirmLabel="Propose removal"
            tone="danger"
            onConfirm={() => setConfirm(false)}
          />
        </State>
      </Specimen>

      <Specimen name="CodeBlock">
        <State is="short">
          <CodeBlock label="Entity id" className="rounded bg-surface-subtle p-2 font-mono text-caption">
            urn:ngsi-ld:AirQualityObserved:ba-01
          </CodeBlock>
        </State>
        <State is="a line too long, scrolls sideways and takes focus">
          <CodeBlock label="Request as curl" className="max-w-xs rounded bg-surface-subtle p-2 font-mono text-caption">
            {`curl -H "Accept: application/ld+json" "https://api.example.sk/ngsi-ld/v1/entities?type=AirQualityObserved&limit=100"`}
          </CodeBlock>
        </State>
      </Specimen>

      <Specimen name="RecordView">
        <State is="an NGSI-LD entity, attributes unwrapped">
          <RecordView
            label="Sample record"
            className="max-w-md rounded bg-surface-subtle p-2"
            value={{
              id: "urn:ngsi-ld:AirQualityObserved:ba-01",
              type: "AirQualityObserved",
              pm10: { type: "Property", value: 12, unitCode: "GQ", observedAt: "2026-10-24T08:00:00Z" },
              refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:ba-01" },
              location: { type: "GeoProperty", value: { type: "Point", coordinates: [17.1077, 48.1486] } },
              address: { type: "Property", value: { streetAddress: "Hlavná 1", addressLocality: "Bratislava" } },
            }}
          />
        </State>
        <State is="empty">
          <RecordView label="Empty record" className="max-w-md rounded bg-surface-subtle p-2" value={{}} />
        </State>
      </Specimen>

      <Specimen name="Icon, Term, SourceLink, ExternalLink, FilePicker, FileDropZone, PermissionGuard">
        <State is="decorative and named">
          <span className="flex items-center gap-2">
            <Icon name="spaces" className="size-5" />
            <Icon name="endpoints" title="Endpoints" className="size-5" />
          </span>
        </State>
        <State is="a word with its definition">
          <p className="text-body">
            A <Term name="endpoint" /> serves a <Term name="contextSpace">Context Space</Term>.
          </p>
        </State>
        <State is="where a manifest lives">
          <SourceLink href="https://git.example.sk/city/org/src/branch/main/space.yaml" label="Open in the repository" />
        </State>
        <State is="off the Portal">
          <ExternalLink href="https://www.etsi.org/">ETSI GS CIM 009</ExternalLink>
        </State>
        <State is="a file to choose">
          <FilePicker label="Choose a model file" accept=".yaml" onFile={() => undefined}>
            <span className="text-body">Drop a .yaml here</span>
          </FilePicker>
        </State>
        <State is="a zone to drop files on">
          <FileDropZone
            label="Files to read"
            button="Drop files here or choose them"
            hint="CSV, JSON or YAML"
            multiple
            onFiles={() => undefined}
          />
        </State>
        <State is="a control the role may not use">
          <PermissionGuard project="helsinki" kind="Endpoint" verb="propose">
            <Button variant="primary">Propose the change</Button>
          </PermissionGuard>
        </State>
      </Specimen>
    </main>
  );
}
