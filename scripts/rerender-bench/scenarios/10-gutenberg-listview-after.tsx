import { createContext, memo, useContext, useMemo, useReducer } from "react";

// Fix: split the context by update frequency, drop the dead prop.
type Ctx = { toggle: (id: string) => void; instanceId: string };
const ListViewContext = createContext<Ctx | null>(null);
const TreeStateContext = createContext<Record<string, boolean>>({});
const BLOCKS = ["a", "b", "c", "d"];

const ListViewBlock = memo(function ListViewBlock({ id }: { id: string }) {
  const { instanceId, toggle } = useContext(ListViewContext)!;
  return <li><button id={"t-" + id} onClick={() => toggle(id)}>{instanceId}:{id}</button></li>;
});
function ListViewBranch() {
  useContext(TreeStateContext);
  return <ul>{BLOCKS.map((id) => <ListViewBlock key={id} id={id} />)}</ul>;
}
function reducer(s: Record<string, boolean>, id: string) { return { ...s, [id]: !s[id] }; }
export default function ListView() {
  const [expanded, toggle] = useReducer(reducer, {});
  const value = useMemo(() => ({ toggle, instanceId: "lv1" }), []);
  return (
    <ListViewContext.Provider value={value}>
      <TreeStateContext.Provider value={expanded}><ListViewBranch /></TreeStateContext.Provider>
    </ListViewContext.Provider>
  );
}
export const interaction = "expand then collapse block a";
export async function interact(ui: any) { await ui.click("#t-a"); await ui.click("#t-a"); }
