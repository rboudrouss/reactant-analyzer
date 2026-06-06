// Next.js-style: `/users/page.tsx` and `/posts/page.tsx` both define `Page`.
// This one has an infinite loop in its mount effect — the analyzer must fire
// `infinite-loop` on THIS Page without confusing it with the other file.

function Page() {
  const [c, setC] = useState(0);
  useEffect(() => {
    setC(c + 1);
  }, [c]);
  return <div>{c}</div>;
}
