import { useState } from "react";

// Fix: the trigger button and the dialog form one component that owns `open`.
function Item({ n }: { n: number }) { return <li>{n}</li>; }
function BigList() { return <ul>{[1, 2, 3, 4].map((n) => <Item key={n} n={n} />)}</ul>; }
function Dialog({ onClose }: { onClose: () => void }) {
  return <div role="dialog"><button id="close" onClick={onClose}>x</button></div>;
}
function DialogTrigger() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button id="open" onClick={() => setOpen(true)}>open</button>
      {open && <Dialog onClose={() => setOpen(false)} />}
    </>
  );
}
export default function Page() {
  return <div><DialogTrigger /><BigList /></div>;
}
export const interaction = "open then close";
export async function interact(ui: any) { await ui.click("#open"); await ui.click("#close"); }
