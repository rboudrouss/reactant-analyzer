import { createContext, useContext, useState } from "react";
import { Tooltip } from "some-ui";

// The state reaches its only consumer by context: `App` and `Page` re-render
// on every write for nothing (#145).
const Query = createContext({ q: "", setQ: (_: string) => {} });
function Search() {
  const { q, setQ } = useContext(Query);
  return <input value={q} onChange={(e) => setQ(e.target.value)} />;
}
function Header() { return <h1>Title</h1>; }
function Page() { return <main><Header /><Search /></main>; }
export function App() {
  const [q, setQ] = useState("");
  return (
    <Query.Provider value={{ q, setQ }}>
      <Header />
      <Page />
    </Query.Provider>
  );
}

// An element the analysis cannot see into may read the context too, so the
// state may be used right below its owner.
const Other = createContext({ q: "", setQ: (_: string) => {} });
function OtherSearch() {
  const { q, setQ } = useContext(Other);
  return <input value={q} onChange={(e) => setQ(e.target.value)} />;
}
function OtherPage() { return <main><Header /><OtherSearch /></main>; }
export function Guarded() {
  const [q, setQ] = useState("");
  return (
    <Other.Provider value={{ q, setQ }}>
      <Tooltip />
      <OtherPage />
    </Other.Provider>
  );
}
