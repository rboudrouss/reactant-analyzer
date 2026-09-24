import { createContext, useContext, useState, type ReactNode } from "react";

// The unrelated-looking child sits inside a provider that receives the
// state: it may read it by context, so nothing is claimed about it.
const Ctx = createContext("");
function Reader() { const v = useContext(Ctx); return <p>{v}</p>; }
function Panel({ children }: { children: ReactNode }) { return <section>{children}</section>; }
export default function App() {
  const [text, setText] = useState("");
  return (
    <Ctx.Provider value={text}>
      <input value={text} onChange={(e) => setText(e.target.value)} />
      <Panel><Reader /></Panel>
    </Ctx.Provider>
  );
}
