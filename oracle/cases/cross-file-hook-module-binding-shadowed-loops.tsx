// @file App.tsx
// oracle: react=loop
// #354, #335. A hook of another file stores its setter in its own file's
// module binding `setter`, and calls a method of its own file's `o` that
// calls it. The component's file binds both names to `const`s of its own:
// the hook's names are still its file's bindings.
import { useTick } from "./hook";

const setter = 0;
const o = { run: () => 0 };

export function App() {
  const n = useTick();
  return <div>{n + setter + o.run()}</div>;
}

// @file hook.tsx
import { useEffect, useState } from "react";

let setter = null;
const o = {
  run() {
    if (setter) setter((c) => c + 1);
  },
};

export function useTick() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setter = setN;
  });
  useEffect(() => {
    o.run();
  });
  return n;
}
