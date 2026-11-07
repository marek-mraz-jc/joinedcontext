/**
 * The Portal's `style-src` takes a `<style>` element only with the answer's nonce (T-5828). The
 * server writes that nonce into `<meta property="csp-nonce">` (vite `html.cspNonce`); this stamps
 * it on every `<style>` the UI's own scripts create, so the stylesheets Monaco and Radix's scroll
 * lock write at run time apply. Markup injected into a page is parsed, not created here, and stays
 * refused. Without a nonce (vite preview, tests) nothing is stamped.
 */
export function stampStyleNonce(doc: Document): void {
  const nonce = doc.querySelector<HTMLMetaElement>("meta[property=csp-nonce]")?.nonce;
  if (!nonce || nonce === "__JC_CSP_NONCE__") return;
  const create = doc.createElement.bind(doc) as (tagName: string, options?: ElementCreationOptions) => HTMLElement;
  doc.createElement = ((tagName: string, options?: ElementCreationOptions) => {
    const element = create(tagName, options);
    if (element instanceof HTMLStyleElement) element.nonce = nonce;
    return element;
  }) as Document["createElement"];
}

stampStyleNonce(document);
