import { useState } from "react";

// State lives in App, but only the input and <Preview> read it.
// Typing re-renders App, Preview AND the unrelated ExpensiveTree subtree.
function Row({ i }: { i: number }) { return <li>row {i}</li>; }
function ExpensiveTree() {
  return <ul>{[0, 1, 2].map((i) => <Row key={i} i={i} />)}</ul>;
}
function Preview({ text }: { text: string }) { return <p>{text}</p>; }

export default function App() {
  const [text, setText] = useState("");
  return (
    <div>
      <input id="q" value={text} onChange={(e) => setText(e.target.value)} />
      <Preview text={text} />
      <ExpensiveTree />
    </div>
  );
}
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#q", "hello"); }
