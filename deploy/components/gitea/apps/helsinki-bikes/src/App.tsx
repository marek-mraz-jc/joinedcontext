import { AppShell } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { Overview } from "./pages/Overview";
import { Stations } from "./pages/Stations";

const PAGES: ShellPage[] = [
  { id: "overview", label: "Overview", render: () => <Overview /> },
  { id: "stations", label: "Stations", render: () => <Stations /> },
];

/**
 * Helsinki's city bikes in the SDK's shell (SDK-39): the numbers for the whole city, and every
 * station on a map and in a table; a station opens in the shell's entity panel (SDK-40).
 */
export default function App() {
  return <AppShell title="Helsinki city bikes" pages={PAGES} language="en" />;
}
