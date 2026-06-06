// Custom hook with an infinite loop — the bug must surface on `Page` after
// inlining (custom hooks bodies are spliced into the calling component's
// fixpoint).

function useData(initial) {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    setValue(value + 1);
  }, [value]);
  return value;
}
