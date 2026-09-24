import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

// Fix: split the context by update frequency.
const UserCtx = createContext("");
const CursorCtx = createContext({ x: 0, y: 0 });
function Provider({ children }: { children: ReactNode }) {
  const [cursor, setCursor] = useState({ x: 0, y: 0 });
  useEffect(() => {
    const h = (e: MouseEvent) => setCursor({ x: e.clientX, y: e.clientY });
    window.addEventListener("mousemove", h);
    return () => window.removeEventListener("mousemove", h);
  }, []);
  return <UserCtx.Provider value="ada"><CursorCtx.Provider value={cursor}>{children}</CursorCtx.Provider></UserCtx.Provider>;
}
function Header() { const user = useContext(UserCtx); return <h1>{user}</h1>; }
function Pointer() { const cursor = useContext(CursorCtx); return <p>{cursor.x}</p>; }
export default function App() { return <Provider><Header /><Pointer /></Provider>; }
export const interaction = "5 mousemoves";
export async function interact(ui: any) { await ui.fireWindow("mousemove", 5); }
