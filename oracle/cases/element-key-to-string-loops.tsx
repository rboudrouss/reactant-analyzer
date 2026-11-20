// oracle: react=loop
// #371: the JSX runtime converts an element's `key` to a string where the
// element is created (`jsx`: `"" + config.key`), in the render, for a
// component's element as for a host one: the `toString` the object holds
// runs there, with the setter.
import { useState } from "react";

function Item() {
  return <span>i</span>;
}

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "k";
    },
  };
  return (
    <div>
      {n}
      <Item key={o as unknown as string} />
    </div>
  );
}
