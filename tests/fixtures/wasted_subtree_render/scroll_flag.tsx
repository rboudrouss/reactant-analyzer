import { useEffect, useState } from "react";

// dub `useScroll`, reduced: every scroll writes a boolean, but React bails out
// on an unchanged value, so the page re-renders only when it flips.
function Nav() { return <nav>{[1, 2, 3].map((i) => <a key={i}>{i}</a>)}</nav>; }
function Links() { return <ul>{[1, 2].map((i) => <Nav key={i} />)}</ul>; }
export default function Header() {
  const [scrolled, setScrolled] = useState(false);
  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 50);
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  return <header className={scrolled ? "shadow" : ""}><Links /></header>;
}
