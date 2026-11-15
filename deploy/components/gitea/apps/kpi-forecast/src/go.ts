/** Opens `url` in this tab: a download the store serves with its own name and type. */
export function go(url: string): void {
  window.location.assign(url);
}
