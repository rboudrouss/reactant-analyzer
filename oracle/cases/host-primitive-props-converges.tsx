// oracle: react=converges
// #371: a host element's props that are strings, numbers or booleans convert
// to themselves, which runs nothing of the program; the handler the element
// holds runs only on a click.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div
      title={"t" + n}
      className={n > 1 ? "a" : "b"}
      data-n={n}
      hidden={n > 5}
      style={{ width: n, color: "red" }}
      onClick={() => setN(n + 1)}
    >
      {n}
    </div>
  );
}
