/** A name the Endpoint can take: the space and the type, lower case, at most 63 characters. */
export function publicName(space: string, type: string): string {
  const base = `${space}-${type}`.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/-+/g, "-");
  return `${base.slice(0, 55).replace(/^-+|-+$/g, "")}-public`;
}
