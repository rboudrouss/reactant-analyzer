// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents fn
// A closure bound to a name, handed as a named prop to a package component
// that calls it from an effect after every commit: React loops. By default
// the row is the Info hand-over (`disclosed`). Under `packagePropsRunOnEvents`
// the prop is read as an event's, the premise is false here, and the loop is
// missed: `fn` by the assumption, its price (ADR-048).
import { useState } from "react";
import { EffectCalls } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const bump = () => setN(n + 1);
  return (
    <div>
      {n}
      <EffectCalls fn={bump} />
    </div>
  );
}
