import { useState } from "react";
import { useSync } from "./useSync";

export function B() {
  const [p, setP] = useState(0);
  const [q, setQ] = useState(0);
  useSync(p, setQ);
  useSync(q, setP);
  return (
    <div>
      {p}
      {q}
    </div>
  );
}
