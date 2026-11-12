// oracle: react=loop
// Two components: the parent's effect on `[location]` bumps a count it hands
// down, and the child's effect on that count navigates to a URL built from it.
// Each navigation makes a new `location`, so the parent bumps again, the child
// navigates again, for ever.
import { useEffect, useState } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";

function Child({ n }: { n: number }) {
  const navigate = useNavigate();
  useEffect(() => {
    navigate("/x/" + n);
  }, [n]);
  return <p>{n}</p>;
}

function Parent() {
  const location = useLocation();
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [location]);
  return <Child n={n} />;
}

export function App() {
  return (
    <MemoryRouter>
      <Parent />
    </MemoryRouter>
  );
}
