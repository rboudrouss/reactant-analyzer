export function Row(props: { checked: boolean; onChange: (c: boolean) => void }) {
  return <div onClick={() => props.onChange(!props.checked)}>{String(props.checked)}</div>;
}
