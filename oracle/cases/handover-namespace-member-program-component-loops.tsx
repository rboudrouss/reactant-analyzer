// oracle: react=loop
// A program component stored on a package's namespace, rendered by member
// name: under module interop the namespace is a mutable object, and `Local`
// calls the prop from an effect. A Warning (`tp`): a namespace member is the
// package's only while no write of the program may have replaced it.
import { useEffect, useState } from "react";
import * as Ext from "ext";

function Local({ onPick }: { onPick: (n: number) => void }) {
  useEffect(() => {
    onPick(Math.random());
  });
  return null;
}

(Ext as any).Comp = Local;

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <Ext.Comp onPick={(x: number) => setS(x)} />
    </div>
  );
}
