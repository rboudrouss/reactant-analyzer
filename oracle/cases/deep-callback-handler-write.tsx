// oracle: react=converges
// #267 negative: the same three-deep write under a JSX handler. The deeper
// walk keeps the listener's class: a user event stands between the render
// and the write, so it is no render write.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  return (
    <button onClick={() => [1].forEach(() => [2].forEach(() => setN(n + 1)))}>
      {n}
    </button>
  );
}
