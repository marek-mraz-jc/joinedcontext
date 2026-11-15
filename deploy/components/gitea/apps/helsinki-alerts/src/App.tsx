import { AppShell, useMe } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { Alerts } from "./pages/Alerts";
import { Overview } from "./pages/Overview";

const PAGES: ShellPage[] = [
  { id: "overview", label: "Overview", render: () => <Overview /> },
  { id: "alerts", label: "Alerts", render: () => <Alerts /> },
];

/**
 * Traffic alerts in the capital region in the SDK's shell (SDK-39): the numbers, and every alert on a
 * map and in a table; an alert opens in the shell's entity panel (SDK-40), signed in as the reader.
 */
export default function App() {
  const user = useMe();
  return <AppShell title="Helsinki traffic alerts" pages={PAGES} language="en" userName={user?.name} />;
}
