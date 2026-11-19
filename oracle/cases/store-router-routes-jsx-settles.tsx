// oracle: react=converges
// `<Routes>` and `<Route>` come from the real library and must reach React
// unchanged: `Routes` accepts only `<Route>` children, by element type, so the
// oracle must not wrap an imported component in its render counter. The program's
// own components are still counted.
import { useState } from "react";
import { MemoryRouter, Routes, Route, Outlet, Link } from "react-router";

function Home() {
  const [n] = useState(0);
  return <p>{n}</p>;
}

function Layout() {
  return (
    <div>
      <Link to="/">home</Link>
      <Outlet />
    </div>
  );
}

export function App() {
  return (
    <MemoryRouter initialEntries={["/"]}>
      <Routes>
        <Route element={<Layout />}>
          <Route path="/" element={<Home />} />
        </Route>
      </Routes>
    </MemoryRouter>
  );
}
