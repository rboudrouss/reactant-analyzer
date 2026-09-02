export function Render4Fires({ items }) {
  return (
    <ul>
      {items.map((it) => (
        <Row key={crypto.randomUUID()} item={it} />
      ))}
    </ul>
  );
}

export function Render4Silent({ items }) {
  return (
    <ul>
      {items.map((it) => (
        <Row key={it.id} item={it} />
      ))}
    </ul>
  );
}
