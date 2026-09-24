import { useWindowSize } from "./use-size";

// Every resize writes a fresh object in the hook: Page and its unrelated
// list re-render on each tick, although Page only reads `isWide`.
function Row({ i }: { i: number }) { return <li>{i}</li>; }
function List() { return <ul>{[1, 2, 3].map((i) => <Row key={i} i={i} />)}</ul>; }
export default function Page() {
  const { isWide } = useWindowSize();
  return <main className={isWide ? "wide" : ""}><List /></main>;
}
