// The help under a field: the first paragraph's whole sentences, as many as fit HELP_CHARS and at
// least one. A cut mid-word ("... If se") read as a broken form (T-4819). A sentence ends at
// . ! or ? before a space, so "a .pem file" or "e.g.x" stays inside its sentence.
export const HELP_CHARS = 240;
export function helpText(text) {
  const paragraph = (text ?? "").split("\n")[0].trim();
  let out = "";
  for (const sentence of paragraph.split(/(?<=[.!?])\s+/)) {
    const next = out ? `${out} ${sentence}` : sentence;
    if (out && next.length > HELP_CHARS) break;
    out = next;
  }
  return out;
}
