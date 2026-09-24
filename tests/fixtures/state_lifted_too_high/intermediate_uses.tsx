import { useState } from "react";

// Layout reads `value` itself (a host attribute), so the state belongs in
// Layout, one level down from App, not in Field.
function Content() { return <article>static</article>; }
function Other() { return <nav>nav</nav>; }
function Field({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <input value={value} onChange={(e) => onChange(e.target.value)} />;
}
function Layout({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <main title={value}><Field value={value} onChange={onChange} /><Content /></main>;
}
export default function App() {
  const [value, setValue] = useState("");
  return <div><Layout value={value} onChange={setValue} /><Other /></div>;
}
