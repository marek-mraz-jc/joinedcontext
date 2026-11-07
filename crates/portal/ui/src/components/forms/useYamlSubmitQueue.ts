import { useEffect, useEffectEvent, useRef, useState } from "react";

/**
 * The YAML view submits what the form view would: the same schema decides (UI-01). One render
 * later: the parent rebuilds its schema and its base manifest from the form it is handed (a rate
 * limit outside the classes, a field the form has no control for), so what is checked and
 * proposed is the document typed, not the render before it (T-0890).
 *
 * `enqueue` holds the form and asks for a render; `consume` runs in the effect after that render,
 * with the props and the schema that render brought.
 */
export function useYamlSubmitQueue<T>(consume: (form: T) => void): (form: T) => void {
  const queued = useRef<T | null>(null);
  const [asked, setAsked] = useState(0);
  const onQueued = useEffectEvent(consume);
  useEffect(() => {
    const form = queued.current;
    if (form === null) {
      return;
    }
    queued.current = null;
    onQueued(form);
  }, [asked]);
  return (form: T) => {
    queued.current = form;
    setAsked((count) => count + 1);
  };
}
