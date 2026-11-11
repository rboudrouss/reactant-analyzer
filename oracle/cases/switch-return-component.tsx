// oracle: react=loop
// A component whose only JSX returns sit in a `switch` was not detected
// (#178): the child was absent from the analysis, so the parent's setter it
// calls during render went unreported. The `if` form was always found.
import { useState } from "react";

function ViaSwitch({ k, onChange }) {
  onChange(k + 1);
  switch (k % 2) {
    case 0:
      return <a>{k}</a>;
    default:
      return <b>{k}</b>;
  }
}

export function App() {
  const [n, setN] = useState(0);
  return <div><ViaSwitch k={n} onChange={setN} /></div>;
}
