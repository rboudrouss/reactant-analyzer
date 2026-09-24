import { useState } from "react";

// Forwarded into list items: one state per item would change the meaning,
// so the home is Wrapper, which maps them, never Row.
function Row({ id, selected, onPick }: { id: number; selected: number; onPick: (id: number) => void }) {
  return <li className={id === selected ? "on" : ""} onClick={() => onPick(id)}>{id}</li>;
}
function Wrapper({ selected, onPick }: { selected: number; onPick: (id: number) => void }) {
  return <ul>{[1, 2, 3].map((id) => <Row key={id} id={id} selected={selected} onPick={onPick} />)}</ul>;
}
function Side() { return <aside>side</aside>; }
export default function App() {
  const [selected, setSelected] = useState(0);
  return <div><Wrapper selected={selected} onPick={setSelected} /><Side /></div>;
}
