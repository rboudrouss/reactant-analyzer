import { useState, useEffect } from 'react';
import * as Ext from 'ext';

function Local({ onPick }: { onPick: (n: number) => void }) {
  useEffect(() => { onPick(Math.random()); });
  return null;
}
// A program component stored on a package object, rendered by member name:
// a namespace is mutable under module interop. A Warning.
(Ext as any).Comp = Local;

export function App() {
  const [s, setS] = useState(0);
  return <div>{s}<Ext.Comp onPick={(x: number) => setS(x)} /></div>;
}
