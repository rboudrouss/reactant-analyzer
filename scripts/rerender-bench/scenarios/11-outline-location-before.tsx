import { createContext, memo, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

// outline/outline#12116 (reduced). SortableTable subscribes to the location
// only to use it inside the sort handler; every navigation re-renders it and
// gives the handler a new identity, defeating memo(Table).
type Loc = { pathname: string };
const LocationCtx = createContext<{ location: Loc; history: { location: Loc; push: (p: string) => void } } | null>(null);
function Router({ children }: { children: ReactNode }) {
  const [location, setLocation] = useState<Loc>({ pathname: "/a" });
  const history = useMemo(() => ({ location, push: (p: string) => setLocation({ pathname: p }) }), [location]);
  return <LocationCtx.Provider value={{ location, history }}>{children}</LocationCtx.Provider>;
}
const Table = memo(function Table({ onChangeSort }: { onChangeSort: (s: string) => void }) {
  return <table><thead><tr><th onClick={() => onChangeSort("name")}>name</th></tr></thead></table>;
});
function SortableTable() {
  const { location, history } = useContext(LocationCtx)!;
  const handleChangeSort = useCallback((sort: string) => history.push(location.pathname + "?sort=" + sort), [location, history]);
  return <Table onChangeSort={handleChangeSort} />;
}
function Nav() {
  const { history } = useContext(LocationCtx)!;
  return <a id="nav" onClick={() => history.push("/b" + Math.random())}>go</a>;
}
export default function App() { return <Router><Nav /><SortableTable /></Router>; }
export const interaction = "navigate twice";
export async function interact(ui: any) { await ui.click("#nav"); await ui.click("#nav"); }
