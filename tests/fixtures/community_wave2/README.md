Fixture pairs for `packs/community/wave2.json` — the seven scenarios the
`calls`, `reads`, `none` and host-element additions (#126, #127) made
expressible.

Every file holds one `…Fires` component (the scenario's "Fires on" snippet) and
one `…Silent` component (its deliberately hard near-miss). `tests/community_packs.rs`
asserts that each rule fires on every `Fires` component and on no `Silent` one,
which is what keeps `docs/campaign/triage-2026-09-02-wave2.md` from being a
claim nobody re-checks.
