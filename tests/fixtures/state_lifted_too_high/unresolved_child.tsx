import { useState } from "react";
import { Field } from "some-ui-lib";

// Field comes from a package the analysis cannot see into: it may use the
// value any way it likes, so nothing is claimed.
function Content() { return <article>static</article>; }
function Layout({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <main><Field value={value} onChange={onChange} /><Content /></main>;
}
export default function App() {
  const [value, setValue] = useState("");
  return <Layout value={value} onChange={setValue} />;
}
