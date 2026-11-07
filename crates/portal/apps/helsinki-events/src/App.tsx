import { AppShell } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { Events } from "./pages/Events";

const PAGES: ShellPage[] = [{ id: "events", label: "Events", render: () => <Events /> }];

/**
 * Helsinki's upcoming events in the SDK's shell (SDK-39): charts, a map and the list, read from the
 * app's own endpoint; an event opens in the shell's entity panel (SDK-40).
 */
export default function App() {
  return <AppShell title="Helsinki events" pages={PAGES} language="en" />;
}
