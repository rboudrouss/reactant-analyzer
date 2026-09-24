import { useState } from "react";

// Prop drilling: App owns `value` only to hand it down through Layout and
// Sidebar to Field, which is also the only writer. Every keystroke re-renders
// the whole chain plus Layout's other child (Content).
function Content() { return <article>static content</article>; }
function Field({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <input id="f" value={value} onChange={(e) => onChange(e.target.value)} />;
}
function Sidebar({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <aside><h2>Filters</h2><Field value={value} onChange={onChange} /></aside>;
}
function Layout({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <main><Sidebar value={value} onChange={onChange} /><Content /></main>;
}
export default function App() {
  const [value, setValue] = useState("");
  return <Layout value={value} onChange={setValue} />;
}
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#f", "hello"); }
