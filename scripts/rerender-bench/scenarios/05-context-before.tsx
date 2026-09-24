import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

// One context mixes a fast field (cursor, mousemove) with a slow one (user).
// Header only reads `user` but re-renders on every mouse move.
type Ctx = { user: string; cursor: { x: number; y: number } };
const AppCtx = createContext<Ctx>({ user: "", cursor: { x: 0, y: 0 } });
function Provider({ children }: { children: ReactNode }) {
  const [cursor, setCursor] = useState({ x: 0, y: 0 });
  useEffect(() => {
    const h = (e: MouseEvent) => setCursor({ x: e.clientX, y: e.clientY });
    window.addEventListener("mousemove", h);
    return () => window.removeEventListener("mousemove", h);
  }, []);
  const value = useMemo(() => ({ user: "ada", cursor }), [cursor]);
  return <AppCtx.Provider value={value}>{children}</AppCtx.Provider>;
}
function Header() { const { user } = useContext(AppCtx); return <h1>{user}</h1>; }
function Pointer() { const { cursor } = useContext(AppCtx); return <p>{cursor.x}</p>; }
export default function App() { return <Provider><Header /><Pointer /></Provider>; }
export const interaction = "5 mousemoves";
export async function interact(ui: any) { await ui.fireWindow("mousemove", 5); }
