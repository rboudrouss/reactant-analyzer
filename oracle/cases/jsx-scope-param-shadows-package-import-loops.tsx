// oracle: react=loop
// #392. A parameter shadows a package import of the same name: `Inner`'s
// `<D/>` is the closure its parent passes, which calls the prop in its
// render. The package's `ClickCalls`, which `App` renders under that name
// itself, is not the component that runs in `Inner`.
import { useState } from "react";
import { ClickCalls as D } from "ext";

function Inner({ D, setS }: { D: any; setS: (n: number) => void }) {
  return <D fn={(x: number) => setS(x)} />;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <Inner
        D={(p: any) => {
          p.fn(Math.random());
          return null;
        }}
        setS={setS}
      />
      <D fn={() => setS(2)} />
    </div>
  );
}
