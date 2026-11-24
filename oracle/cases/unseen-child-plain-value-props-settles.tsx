// oracle: react=converges
// #325. A child the analysis does not resolve receives no function: nothing it
// is handed can write the parent's state, so nothing is claimed.
import { useState } from "react";

function Badge({ count, label }: { count: number; label: string }) {
  return (
    <span>
      {label}
      {count}
    </span>
  );
}

const ALL = [Badge];

export function App() {
  const [n] = useState(0);
  const B = ALL[0];
  return <B count={n} label="x" />;
}
