import React, { useState, useEffect, useMemo, useCallback, useRef } from "react";

// ── MetricCard ────────────────────────────────────────────────────────────────
//
// Clean: useRef for a timer handle (stable), correct effect deps.

function MetricCard(props) {
  const [value, setValue] = useState(0);
  const [loading, setLoading] = useState(true);
  const intervalRef = useRef(null);

  useEffect(() => {
    setLoading(true);
    fetch("/api/metrics")
      .then((r) => r.json())
      .then((data) => {
        setValue(data.value);
        setLoading(false);
      });
  }, []);

  useEffect(() => {
    intervalRef.current = setInterval(() => {
      fetch("/api/metrics/live")
        .then((r) => r.json())
        .then((data) => setValue(data.value));
    }, 5000);
    return () => clearInterval(intervalRef.current);
  }, []);

  if (loading) {
    return <div>Loading…</div>;
  }

  return <div>{value}</div>;
}

// ── DataTable ─────────────────────────────────────────────────────────────────
//
// Clean: useMemo for sorted rows, useCallback for sort toggle.

function DataTable(props) {
  const [rows, setRows] = useState([]);
  const [sortKey, setSortKey] = useState("name");
  const [sortAsc, setSortAsc] = useState(true);

  useEffect(() => {
    fetch("/api/table")
      .then((r) => r.json())
      .then((data) => setRows(data));
  }, []);

  const sorted = useMemo(() => {
    const copy = rows.slice();
    copy.sort((a, b) => {
      if (a[sortKey] < b[sortKey]) return sortAsc ? -1 : 1;
      if (a[sortKey] > b[sortKey]) return sortAsc ? 1 : -1;
      return 0;
    });
    return copy;
  }, [rows, sortKey, sortAsc]);

  const toggleSort = useCallback(
    (key) => {
      if (key === sortKey) {
        setSortAsc((v) => !v);
      } else {
        setSortKey(key);
        setSortAsc(true);
      }
    },
    [sortKey]
  );

  return (
    <table>
      <thead>
        <tr>
          <th onClick={() => toggleSort("name")}>Name</th>
          <th onClick={() => toggleSort("value")}>Value</th>
        </tr>
      </thead>
      <tbody>
        {sorted.map((row) => (
          <tr key={row.id}>
            <td>{row.name}</td>
            <td>{row.value}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

// ── AlertBanner ──────────────────────────────────────────────────────────────
//
// Bug: useState called inside an if block → conditional-hook.

function AlertBanner(props) {
  const [dismissed, setDismissed] = useState(false);

  // ❌ conditional-hook: useState inside an if.
  if (!dismissed) {
    const [severity, setSeverity] = useState("info");
  }

  return dismissed ? null : (
    <div>
      <span>Alert!</span>
      <button onClick={() => setDismissed(true)}>×</button>
    </div>
  );
}

// ── LiveChart ─────────────────────────────────────────────────────────────────
//
// Bug: setData([]) in effect without data in deps may cause stale closure.
// Also calls setData with an object literal → Unstable update.

function LiveChart(props) {
  const [data, setData] = useState([]);
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    if (paused) return;
    const id = setInterval(() => {
      setData((prev) => [...prev, Math.random()]);
    }, 1000);
    return () => clearInterval(id);
  }, [paused]);

  const avg = useMemo(() => {
    if (data.length === 0) return 0;
    return data.reduce((a, b) => a + b, 0) / data.length;
  }, [data]);

  return (
    <div>
      <p>avg: {avg}</p>
      <button onClick={() => setPaused((p) => !p)}>
        {paused ? "Resume" : "Pause"}
      </button>
      <button onClick={() => setData([])}>Clear</button>
    </div>
  );
}
