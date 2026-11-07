/**
 * UI-46: what the browser keeps, and what leaving takes with it.
 *
 * Storage here holds working state, not a session: which endpoints the assistant was pointed at,
 * which run was open, how a grid was arranged for a project. None of it authenticates anything,
 * but all of it names the projects, endpoints and runs one person was working on, and a Portal is
 * signed into from shared machines. Signing out empties it.
 *
 * Two keys stay: the language and the theme are how this browser is set up rather than who was
 * using it, and keeping them means the sign-in page comes back the way it was left.
 */
const KEPT = new Set(["jc-lang", "jc-theme"]);

/** Empties everything this origin stored except the machine's own display preferences. */
export function clearBrowserState(win: Window = window): void {
  try {
    win.sessionStorage.clear();
    // Collected first: removing while iterating the live `localStorage` skips keys.
    const doomed = Object.keys(win.localStorage).filter((key) => !KEPT.has(key));
    for (const key of doomed) {
      win.localStorage.removeItem(key);
    }
  } catch {
    // Storage blocked — a private window, or a browser refusing it to a framed origin. There is
    // then nothing stored to clear, and signing out must not fail over it.
  }
}
