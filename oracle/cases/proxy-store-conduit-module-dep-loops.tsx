// @file App.tsx
// oracle: react=loop
// #336. The proxy is made by a module the reading component does not import,
// and which does not import it: `install` puts it in a store both import, and
// the entry loads both. Every read of `store.o.p` is a new object, so the
// effect re-runs forever. A narrowing of the accessor fact by the import
// graph between the defining module and the component would miss it.
import "./install";
import { Child } from "./child";

export function App() {
  return <Child />;
}

// @file store.tsx
export const store: { o: any } = { o: { p: 1 } };

// @file install.tsx
import { store } from "./store";

store.o = new Proxy({}, { get: () => ({}) });

// @file child.tsx
import { useEffect, useState } from "react";
import { store } from "./store";

export function Child() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [store.o.p]);
  return <div>{n}</div>;
}
