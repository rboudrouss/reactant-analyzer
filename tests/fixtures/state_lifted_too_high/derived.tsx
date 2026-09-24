import { useState } from "react";

// A value derived from the state (and a closure over the setter) is still the
// state: only Search uses it.
function Results() { return <ul><li>r</li></ul>; }
function Search({ query, onQuery }: { query: string; onQuery: (q: string) => void }) {
  return <input value={query} onChange={(e) => onQuery(e.target.value)} />;
}
export default function Page() {
  const [q, setQ] = useState("");
  const trimmed = q.trim().toLowerCase();
  const handle = (v: string) => setQ(v);
  return <div><Search query={trimmed} onQuery={handle} /><Results /></div>;
}
