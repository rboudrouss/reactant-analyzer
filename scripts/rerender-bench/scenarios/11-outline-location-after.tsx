import { createContext, memo, useCallback, useContext, useRef, useState, type ReactNode } from "react";

// Fix: read the location at call time through a stable history object; the
// table no longer subscribes to the changing location.
type Loc = { pathname: string };
type History = { location: Loc; push: (p: string) => void };
const HistoryCtx = createContext<History | null>(null);
const LocationCtx = createContext<Loc>({ pathname: "/" });
function Router({ children }: { children: ReactNode }) {
  const [location, setLocation] = useState<Loc>({ pathname: "/a" });
  const history = useRef<History>({ location, push: (p) => { history.current.location = { pathname: p }; setLocation({ pathname: p }); } });
  return <HistoryCtx.Provider value={history.current}><LocationCtx.Provider value={location}>{children}</LocationCtx.Provider></HistoryCtx.Provider>;
}
const Table = memo(function Table({ onChangeSort }: { onChangeSort: (s: string) => void }) {
  return <table><thead><tr><th onClick={() => onChangeSort("name")}>name</th></tr></thead></table>;
});
function SortableTable() {
  const history = useContext(HistoryCtx)!;
  const handleChangeSort = useCallback((sort: string) => history.push(history.location.pathname + "?sort=" + sort), [history]);
  return <Table onChangeSort={handleChangeSort} />;
}
function Nav() {
  const history = useContext(HistoryCtx)!;
  return <a id="nav" onClick={() => history.push("/b" + Math.random())}>go</a>;
}
export default function App() { return <Router><Nav /><SortableTable /></Router>; }
export const interaction = "navigate twice";
export async function interact(ui: any) { await ui.click("#nav"); await ui.click("#nav"); }
