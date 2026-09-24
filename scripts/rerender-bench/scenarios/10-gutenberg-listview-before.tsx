import { createContext, memo, useContext, useMemo, useReducer } from "react";

// WordPress/gutenberg#81159 (reduced). One memoised context value mixes the
// fast expansion state with stable members; memo rows read only the stable
// members, and also receive a `listPosition` prop they never read.
type Ctx = { expanded: Record<string, boolean>; toggle: (id: string) => void; instanceId: string };
const ListViewContext = createContext<Ctx | null>(null);
const BLOCKS = ["a", "b", "c", "d"];

const ListViewBlock = memo(function ListViewBlock({ id }: { id: string; listPosition: number }) {
  const { instanceId, toggle } = useContext(ListViewContext)!;
  return <li><button id={"t-" + id} onClick={() => toggle(id)}>{instanceId}:{id}</button></li>;
});
function ListViewBranch() {
  const { expanded } = useContext(ListViewContext)!;
  let pos = 0;
  return (
    <ul>
      {BLOCKS.map((id) => { pos += expanded[id] ? 2 : 1; return <ListViewBlock key={id} id={id} listPosition={pos} />; })}
    </ul>
  );
}
function reducer(s: Record<string, boolean>, id: string) { return { ...s, [id]: !s[id] }; }
export default function ListView() {
  const [expanded, toggle] = useReducer(reducer, {});
  const value = useMemo(() => ({ expanded, toggle, instanceId: "lv1" }), [expanded]);
  return <ListViewContext.Provider value={value}><ListViewBranch /></ListViewContext.Provider>;
}
export const interaction = "expand then collapse block a";
export async function interact(ui: any) { await ui.click("#t-a"); await ui.click("#t-a"); }
