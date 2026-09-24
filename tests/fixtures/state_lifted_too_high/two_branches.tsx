import { useState } from "react";

// The value goes to one child and the setter to another: the state is
// already at the lowest common ancestor.
function Input({ onChange }: { onChange: (v: string) => void }) {
  return <input onChange={(e) => onChange(e.target.value)} />;
}
function Preview({ text }: { text: string }) { return <p>{text}</p>; }
function Pane({ children }: { children: React.ReactNode }) { return <section>{children}</section>; }
export default function App() {
  const [text, setText] = useState("");
  return <div><Pane><Input onChange={setText} /></Pane><Preview text={text} /></div>;
}
