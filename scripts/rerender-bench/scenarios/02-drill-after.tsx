import { useState } from "react";

// Fix: nobody above Field reads `value`, so it belongs in Field.
function Content() { return <article>static content</article>; }
function Field() {
  const [value, setValue] = useState("");
  return <input id="f" value={value} onChange={(e) => setValue(e.target.value)} />;
}
function Sidebar() { return <aside><h2>Filters</h2><Field /></aside>; }
function Layout() { return <main><Sidebar /><Content /></main>; }
export default function App() { return <Layout />; }
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#f", "hello"); }
