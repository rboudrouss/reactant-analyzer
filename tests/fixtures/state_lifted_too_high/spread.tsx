import { useState } from "react";

// Forwarded through a spread at each level: still only Field uses it.
function Content() { return <article>static</article>; }
function Field(props: { value: string; onChange: (v: string) => void }) {
  return <input value={props.value} onChange={(e) => props.onChange(e.target.value)} />;
}
function Layout(props: { value: string; onChange: (v: string) => void }) {
  return <main><Field {...props} /><Content /></main>;
}
export default function App() {
  const [value, setValue] = useState("");
  const shared = { value, onChange: setValue };
  return <Layout {...shared} />;
}
