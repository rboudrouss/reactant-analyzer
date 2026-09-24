import { useState } from "react";

// The owner's own submit button reads the state in its handler: the owner
// uses it, nothing to move.
function Field({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <input value={value} onChange={(e) => onChange(e.target.value)} />;
}
function Other() { return <nav>nav</nav>; }
export default function Form({ onSave }: { onSave: (v: string) => void }) {
  const [value, setValue] = useState("");
  return <form><Field value={value} onChange={setValue} /><Other /><button onClick={() => onSave(value)}>save</button></form>;
}
