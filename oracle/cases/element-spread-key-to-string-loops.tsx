// oracle: react=loop
// #371: a spread that holds `key` gives the element its key, which the JSX
// runtime converts to a string where the element is created (`jsx`:
// `hasValidKey(config)`, then `"" + config.key`): the `toString` the object
// holds runs in the render, with the setter.
import { useState } from "react";

function Item(props: { v: number }) {
  return <span>{props.v}</span>;
}

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "k";
    },
  };
  const p = { v: 1, key: o as unknown as string };
  return (
    <div>
      {n}
      <Item {...p} />
    </div>
  );
}
