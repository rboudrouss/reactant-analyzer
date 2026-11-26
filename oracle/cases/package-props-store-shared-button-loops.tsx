// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents fn
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers fn
// W6's t1n, the neighbour of `package-props-store-shared-button-settles`: the
// Button under Scroller is the one whose package calls its prop after every
// commit, and the prop navigates. Each navigation is a new `location`,
// Scroller re-renders, the package calls the prop again: React loops, and the
// cycle row claims it by default (`tp`). Under `packagePropsRunOnEvents` the
// navigation is read as an event's, the premise is false for this package,
// and the loop is missed: `fn` by the assumption, its price (ADR-048).
import { useEffect, useState } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { ClickCalls, EffectCalls } from "ext";

function Button(props: any) {
  return props.auto ? <EffectCalls fn={props.onClick} /> : <ClickCalls fn={props.onClick} />;
}

function Nav() {
  return <Button onClick={() => window.scrollTo(0, 0)} />;
}

function Scroller() {
  const location = useLocation();
  const navigate = useNavigate();
  const [atHome, setAtHome] = useState(true);
  useEffect(() => {
    setAtHome(location.pathname !== "/never");
  }, [location]);
  return atHome ? <Button auto onClick={() => navigate("/y")} /> : null;
}

export function App() {
  return (
    <MemoryRouter>
      <Nav />
      <Scroller />
    </MemoryRouter>
  );
}
