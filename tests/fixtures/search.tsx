import React, { useState, useEffect, useMemo } from "react";

// ── SearchBar ─────────────────────────────────────────────────────────────────
//
// Bug: hook (useState for error) is inside an if block → conditional-hook.

function SearchBar(props) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState([]);

  // ❌ conditional-hook: useState called inside an if block.
  if (query.length > 2) {
    const [error, setError] = useState(null);
  }

  useEffect(() => {
    if (query) {
      fetch("/api/search?q=" + query)
        .then((r) => r.json())
        .then((data) => setResults(data));
    }
  }, [query]);

  return (
    <div>
      <input value={query} onChange={(e) => setQuery(e.target.value)} />
      <ul>
        {results.map((r) => (
          <li key={r.id}>{r.name}</li>
        ))}
      </ul>
    </div>
  );
}

// ── FilteredList ─────────────────────────────────────────────────────────────
//
// Clean: useMemo correctly depends on items and filter.

function FilteredList(props) {
  const [filter, setFilter] = useState("");
  const [items, setItems] = useState([]);

  useEffect(() => {
    fetch("/api/items")
      .then((r) => r.json())
      .then((data) => setItems(data));
  }, []);

  // useMemo with correct deps.
  const visible = useMemo(
    () => items.filter((item) => item.name.includes(filter)),
    [items, filter]
  );

  return (
    <div>
      <input
        placeholder="Filter..."
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />
      <ul>
        {visible.map((item) => (
          <li key={item.id}>{item.name}</li>
        ))}
      </ul>
    </div>
  );
}

// ── AutocompleteInput ─────────────────────────────────────────────────────────
//
// Bug: useEffect with deps [filter] uses `baseUrl` which is a local binding
// (not in deps). Depending on analysis, may trigger missing-deps.

function AutocompleteInput(props) {
  const [value, setvalue] = useState("");
  const [suggestions, setSuggestions] = useState([]);
  const baseUrl = "/api/autocomplete";

  useEffect(() => {
    if (value.length > 1) {
      fetch(baseUrl + "?q=" + value)
        .then((r) => r.json())
        .then((data) => setSuggestions(data));
    }
  }, [value]);

  return (
    <div>
      <input value={value} onChange={(e) => setvalue(e.target.value)} />
      {suggestions.map((s) => (
        <div key={s}>{s}</div>
      ))}
    </div>
  );
}
