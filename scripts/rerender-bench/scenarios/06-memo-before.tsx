import { memo, useState } from "react";

// memo(List) is defeated by a fresh inline callback: each keystroke in the
// filter re-renders List and every Row.
const Row = memo(function Row({ id, onPick }: { id: number; onPick: (id: number) => void }) {
  return <li onClick={() => onPick(id)}>{id}</li>;
});
const List = memo(function List({ onPick }: { onPick: (id: number) => void }) {
  return <ul>{[1, 2, 3].map((id) => <Row key={id} id={id} onPick={onPick} />)}</ul>;
});
export default function App() {
  const [q, setQ] = useState("");
  const [picked, setPicked] = useState(0);
  return (
    <div>
      <input id="q" value={q} onChange={(e) => setQ(e.target.value)} />
      <span>{picked}</span>
      <List onPick={(id) => setPicked(id)} />
    </div>
  );
}
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#q", "hello"); }
