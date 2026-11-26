// oracle: react=converges
// oracle: assume=packagePropsRunOnEvents tn
// W6's t1 (#420's store side): a shared program Button hands its `onClick`
// to a package component that calls it on a click only. One caller passes a
// navigating closure, another (Scroller) one that only scrolls; Scroller's
// effect on `[location]` writes the slot that guards its Button. Nothing
// navigates, React settles. By default the shared Button's joined render
// "writes the store" and the cycle is a Warning (`fp`); under
// `packagePropsRunOnEvents` the navigation is an event's and the row leaves.
import { useEffect, useState } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { ClickCalls } from "ext";

function Button(props: any) {
  return <ClickCalls fn={props.onClick} />;
}

function Nav() {
  const navigate = useNavigate();
  return <Button onClick={() => navigate("/x")} />;
}

function Scroller() {
  const location = useLocation();
  const [atHome, setAtHome] = useState(false);
  useEffect(() => {
    setAtHome(location.pathname === "/");
  }, [location]);
  return atHome ? <Button onClick={() => window.scrollTo(0, 0)} /> : null;
}

export function App() {
  return (
    <MemoryRouter>
      <Nav />
      <Scroller />
    </MemoryRouter>
  );
}
