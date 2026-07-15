// Custom hook with an infinite render loop: setValue(value + 1) with
// [value] as deps — value widens to [0, +∞) in the caller's fixpoint.

function useData(initial) {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    setValue(value + 1);
  }, [value]);
  return value;
}
