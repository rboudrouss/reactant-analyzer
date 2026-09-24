import { useState } from "react";

// Fix: move the state down into the smallest component that owns every reader
// and every writer (the input + Preview). App and ExpensiveTree stop re-rendering.
function Row({ i }: { i: number }) { return <li>row {i}</li>; }
function ExpensiveTree() {
  return <ul>{[0, 1, 2].map((i) => <Row key={i} i={i} />)}</ul>;
}
function Preview({ text }: { text: string }) { return <p>{text}</p>; }
function Editor() {
  const [text, setText] = useState("");
  return (
    <>
      <input id="q" value={text} onChange={(e) => setText(e.target.value)} />
      <Preview text={text} />
    </>
  );
}

export default function App() {
  return (
    <div>
      <Editor />
      <ExpensiveTree />
    </div>
  );
}
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#q", "hello"); }
