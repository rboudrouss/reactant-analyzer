import { useState } from "react";

export function Async9Fires() {
  const [name, setName] = useState("");
  return <input value={name} placeholder="name" />;
}

export function Async9Silent() {
  const [name, setName] = useState("");
  return <input value={name} onChange={(e) => setName(e.target.value)} />;
}
