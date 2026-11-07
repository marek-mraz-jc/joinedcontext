// A small NGSI-LD `q` matcher for e2e stubs (T-6328): a filtered shot must show the narrowed rows.
// It covers what the grid sends: clauses joined by `;`, == (one value or a list), !=, >, >=, <, <=,
// ~= (regex), `attr[*]` for a LanguageProperty and a bare `attr` / `!attr` for has a value / empty.

type Row = Record<string, unknown> & { id: string };

function valueOf(row: Row, attr: string): unknown {
  const member = row[attr.replace(/\[\*\]$/, "")] as { value?: unknown; languageMap?: Record<string, string> } | undefined;
  if (member === undefined) return undefined;
  if (member.languageMap) return Object.values(member.languageMap);
  return member.value;
}

function parseValue(text: string): string | number {
  if (text.startsWith('"')) return JSON.parse(text) as string;
  const n = Number(text);
  return Number.isNaN(n) ? text : n;
}

/** The values of a list (`"a","b"`), split outside quotes. */
function parseList(text: string): (string | number)[] {
  const parts = text.match(/"(?:[^"\\]|\\.)*"|[^,]+/g) ?? [];
  return parts.map(parseValue);
}

function clause(row: Row, text: string): boolean {
  const m = /^(!?)([\w[\]*.]+?)(?:(==|!=|>=|<=|~=|>|<)(.*))?$/.exec(text);
  if (!m) return true;
  const [, not, attr, op, rest = ""] = m;
  const actual = valueOf(row, attr);
  if (!op) return not ? actual === undefined : actual !== undefined;
  const values = Array.isArray(actual) ? actual : [actual];
  return values.some((v) => {
    if (v === undefined) return false;
    if (op === "==") return parseList(rest).some((want) => String(want) === String(v));
    if (op === "!=") return !parseList(rest).some((want) => String(want) === String(v));
    if (op === "~=") return new RegExp(parseValue(rest) as string).test(String(v));
    const want = parseValue(rest);
    const [a, b] = typeof want === "number" ? [Number(v), want] : [String(v), String(want)];
    return op === ">" ? a > b : op === ">=" ? a >= b : op === "<" ? a < b : a <= b;
  });
}

/** The rows a request's `q` and `idPattern` keep. */
export function narrowed<T extends Row>(rows: T[], url: URL): T[] {
  const q = url.searchParams.get("q");
  const idPattern = url.searchParams.get("idPattern");
  const clauses = q ? q.split(";") : [];
  return rows.filter((row) => (!idPattern || new RegExp(idPattern).test(row.id)) && clauses.every((c) => clause(row, c)));
}
