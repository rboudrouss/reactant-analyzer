import { useState } from "react";
import { useThing } from "@/hooks/useThing";

// Nothing wrong here. The point of the fixture is that `useThing` is behind an
// alias reactant cannot resolve, so silence about this component is not proof.
export function App() {
  const [n, setN] = useState(0);
  const thing = useThing();
  return <button onClick={() => setN(n + 1)}>{thing}{n}</button>;
}
