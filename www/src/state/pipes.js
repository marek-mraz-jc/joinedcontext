// Pipeline timers. A tick counts ONLY when the broker accepted the write —
// failures land in the request log (red rows), never in the counter.
import { batchUpsert, listEntities } from "../broker/api.js";
import { uuid } from "../uuid.js";
import { TYPES } from "../model.js";
import { board, burst, emit, refreshSpace, save } from "./board.js";

const timers = new Map();

// Rolling retention via NGSI-LD itself (4.22 transient attributes): every
// simulated reading expires 10 minutes after it was produced, so the broker's
// own GC prunes old temporal instances — the in-browser store stays bounded
// without any custom cleanup. Each tick renews the live attribute.
const RETENTION_MS = 10 * 60_000;
const expiry = () => new Date(Date.now() + RETENTION_MS).toISOString();

export function startPipe(p) {
  stopTimer(p.id);
  timers.set(p.id, setInterval(() => tickPipe(p).catch(() => {}), p.secs * 1000));
}
export const stopTimer = (id) => {
  clearInterval(timers.get(id));
  timers.delete(id);
};

export function startAllPipes() {
  for (const p of board.pipes) if (p.running) startPipe(p);
}

export function togglePipe(id) {
  const p = board.pipes.find((x) => x.id === id);
  if (!p) return;
  p.running = !p.running;
  p.running ? startPipe(p) : stopTimer(id);
  save("antares.pipes", board.pipes);
  emit();
}

export function deletePipe(id) {
  stopTimer(id);
  board.pipes = board.pipes.filter((x) => x.id !== id);
  delete board.positions[`p:${id}`];
  save("antares.pipes", board.pipes);
  save("antares.pos", board.positions);
  emit();
}

export function addPipe(spec) {
  const p = { id: uuid().slice(0, 8), running: true, ticks: 0, ...spec };
  board.pipes.push(p);
  save("antares.pipes", board.pipes);
  startPipe(p);
  emit();
  return p;
}

export async function tickPipe(p) {
  let moved = 1;
  if (p.kind === "source") {
    // One stable entity per simulated device, fresh reading per tick. An
    // upsert in update mode holds in every state the entity can be in:
    // absent, present, or present with its reading expired (4.22) after the
    // page stayed closed past the retention window, where Partial Attribute
    // Update answers 404 and Create Entity answers 409.
    const t = TYPES[p.type];
    const up = await batchUpsert(
      p.into,
      [
        {
          id: `urn:ngsi-ld:${p.type}:pipe-${p.id}`,
          type: p.type,
          [t.attr]: {
            type: "Property",
            value: t.gen(Date.now()),
            observedAt: new Date().toISOString(),
            expiresAt: expiry(),
          },
        },
      ],
      "update",
    );
    if (up.status !== 201 && up.status !== 204) return; // rejected — no tick, log has the row
  } else {
    const list = await listEntities(p.from, { local: true, type: p.type });
    if (!list.length) return; // nothing moved — no burst, no tick
    const up = await batchUpsert(p.into, list);
    if (!up.ok && up.status !== 207) return;
    moved = list.length;
  }
  p.ticks = (p.ticks ?? 0) + 1;
  save("antares.pipes", board.pipes);
  burst(`pipe:${p.id}`, moved);
  await refreshSpace(p.into);
  emit();
}
