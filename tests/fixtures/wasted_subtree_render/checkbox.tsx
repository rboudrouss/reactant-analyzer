import { useState } from "react";

// `change` on a checkbox is one event per click, not per keystroke.
function Row({ i }: { i: number }) { return <li>{i}</li>; }
function List() { return <ul>{[1, 2, 3].map((i) => <Row key={i} i={i} />)}</ul>; }
export default function Settings() {
  const [on, setOn] = useState(false);
  return <div><input type="checkbox" checked={on} onChange={(e) => setOn(e.target.checked)} /><List /></div>;
}
