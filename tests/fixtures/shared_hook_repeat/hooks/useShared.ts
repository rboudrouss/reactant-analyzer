// One defect, in one shared hook. The three components below each inline it,
// so the rule pass honestly produces the finding three times — once per
// consumer, all three naming this file and this line. #129: the human report
// prints that location once and says how many components reach it.

function useShared(initial) {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    setValue(value + 1);
  }, [value]);
  return value;
}
