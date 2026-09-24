import { useState } from "react";

// Click-driven: the page owns `open` for a dialog; opening it re-renders the
// whole page, including the list that has nothing to do with the dialog.
function Item({ n }: { n: number }) { return <li>{n}</li>; }
function BigList() { return <ul>{[1, 2, 3, 4].map((n) => <Item key={n} n={n} />)}</ul>; }
function Dialog({ onClose }: { onClose: () => void }) {
  return <div role="dialog"><button id="close" onClick={onClose}>x</button></div>;
}
export default function Page() {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button id="open" onClick={() => setOpen(true)}>open</button>
      {open && <Dialog onClose={() => setOpen(false)} />}
      <BigList />
    </div>
  );
}
export const interaction = "open then close";
export async function interact(ui: any) { await ui.click("#open"); await ui.click("#close"); }
