import { useMemo, useState } from "react";
import { makeCols } from "ext";
function Child({ setView }: { setView: (v: number) => void }) {
  const cols = useMemo(() => makeCols({ setView }), [setView]);
  return <div>{cols}</div>;
}
export function Parent() {
  const [v, setView] = useState(0);
  return <div>{v}<Child setView={setView} /></div>;
}
