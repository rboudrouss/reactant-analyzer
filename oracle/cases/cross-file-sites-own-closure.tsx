// @file App.tsx
// oracle: react=converges
// #247 (corpus: memos PagedMemoList). Each file numbers its allocation sites
// from zero, and the child's heap holds the parent's: `props.render` pointed
// at the child's own `apply`, the closure the parent's literal shared an id
// with, and its write read as a render write. A component's sites are its
// own now; the parent's renderer writes nothing.
import { List } from "./List";

export function App() {
  return <List render={(x: number) => <b key={x}>{x}</b>} />;
}

// @file List.tsx
import { useLayoutEffect, useState, type ReactElement } from "react";

export function List(props: { render: (x: number) => ReactElement }) {
  const [wide, setWide] = useState(false);
  useLayoutEffect(() => {
    const apply = () => setWide(true);
    apply();
  }, []);
  return (
    <div>
      {[1, 2].map((x) => props.render(x))}
      {String(wide)}
    </div>
  );
}
