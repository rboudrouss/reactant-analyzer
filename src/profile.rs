//! Where the time goes (`REACTANT_PROFILE=1`, on stderr).
//!
//! Off by default; off, every entry point is one load of a cached bool. On,
//! the run keeps a stack of phases and charges each its exclusive time,
//! counts the body runs by nesting depth and by the fixpoint pass that
//! started them, and records each component analysis (root, child, probe,
//! intra) with the size of the heap it was handed. At exit it prints the
//! tables; every 30 s it prints one line (total runs, the top phases, the
//! analysis stack), so a run killed by a time limit still says what it was
//! doing.
//!
//! One analysis runs on one thread, so the state is a thread-local.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::ir::ComponentId;

/// The switch: `REACTANT_PROFILE=1`, read once.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        // `wasm32-unknown-unknown` has no clock.
        !cfg!(target_arch = "wasm32")
            && std::env::var_os("REACTANT_PROFILE").is_some_and(|v| v == "1")
    })
}

/// How often the live line is printed.
const LIVE_EVERY: Duration = Duration::from_secs(30);

/// One open phase: what its time is charged to, and the fixpoint pass the
/// body runs under it count against.
#[derive(Clone, Copy)]
struct Frame {
    phase: &'static str,
    origin: &'static str,
}

/// One open component analysis.
struct Open {
    component: ComponentId,
    kind: &'static str,
    start: Instant,
}

/// The analyses of one (component, kind).
#[derive(Default)]
struct Analyses {
    n: u64,
    inclusive: Duration,
    runs: u64,
    cells: u64,
    max_cells: u64,
    closures: u64,
    max_closures: u64,
}

#[derive(Default)]
struct Profile {
    stack: Vec<Frame>,
    since: Option<Instant>,
    last_live: Option<Instant>,
    exclusive: HashMap<&'static str, Duration>,
    runs: u64,
    runs_by_depth: [u64; 6],
    runs_by_origin: HashMap<&'static str, u64>,
    escaped_owned: u64,
    escaped_foreign: u64,
    names: HashMap<ComponentId, String>,
    open: Vec<Open>,
    analyses: HashMap<(ComponentId, &'static str), Analyses>,
    sites: HashMap<(ComponentId, ComponentId, u32, u32), u64>,
}

thread_local! {
    static PROFILE: RefCell<Profile> = RefCell::new(Profile::default());
}

impl Profile {
    /// Charge the time since the last switch to the open phase, and print
    /// the live line when it is due.
    fn switch(&mut self) {
        let now = Instant::now();
        if let (Some(since), Some(top)) = (self.since, self.stack.last()) {
            *self.exclusive.entry(top.phase).or_default() += now - since;
        }
        self.since = Some(now);
        match self.last_live {
            Some(t) if now - t < LIVE_EVERY => {}
            Some(_) => {
                self.last_live = Some(now);
                self.live();
            }
            None => self.last_live = Some(now),
        }
    }

    fn name(&self, c: ComponentId) -> String {
        match self.names.get(&c) {
            Some(n) => format!("{n}#{}", c.index()),
            None => format!("#{}", c.index()),
        }
    }

    fn live(&self) {
        let top: Vec<String> = top_n(&self.exclusive, 6)
            .into_iter()
            .map(|(p, d)| format!("{p}={}ms", d.as_millis()))
            .collect();
        let stack: Vec<String> = self
            .open
            .iter()
            .map(|o| format!("{}/{}", self.name(o.component), o.kind))
            .collect();
        eprintln!(
            "[profile] live: runs={} top=[{}] analyses=[{}]",
            self.runs,
            top.join(" "),
            stack.join(" > ")
        );
    }

    fn report(&self) {
        let ms = |d: &Duration| d.as_secs_f64() * 1e3;
        let total: Duration = self.exclusive.values().sum();
        eprintln!(
            "[profile] total {:.0} ms, exclusive time per phase:",
            ms(&total)
        );
        for (p, d) in top_n(&self.exclusive, usize::MAX) {
            eprintln!("[profile] phase {p} {:.0} ms", ms(&d));
        }
        eprintln!(
            "[profile] body runs {} by depth {:?}",
            self.runs, self.runs_by_depth
        );
        for (o, n) in top_n(&self.runs_by_origin, usize::MAX) {
            eprintln!("[profile] body runs from {o} {n}");
        }
        eprintln!(
            "[profile] escaped closures run: {} by their owner, {} by another component",
            self.escaped_owned, self.escaped_foreign
        );
        let mut kinds: HashMap<&'static str, (u64, u64)> = HashMap::new();
        for ((_, kind), a) in &self.analyses {
            let k = kinds.entry(*kind).or_default();
            k.0 += a.n;
            k.1 += a.runs;
        }
        let mut kinds: Vec<_> = kinds.into_iter().collect();
        kinds.sort();
        for (kind, (n, runs)) in kinds {
            eprintln!("[profile] analyses {kind} n={n} runs={runs}");
        }
        let mut heavy: Vec<_> = self.analyses.iter().collect();
        heavy.sort_by(|a, b| b.1.inclusive.cmp(&a.1.inclusive).then(a.0.cmp(b.0)));
        for ((c, kind), a) in heavy.into_iter().take(15) {
            eprintln!(
                "[profile] heavy {} {kind} n={} incl={:.0}ms runs={} input closures avg={} max={}, cells avg={} max={}",
                self.name(*c),
                a.n,
                ms(&a.inclusive),
                a.runs,
                a.closures / a.n.max(1),
                a.max_closures,
                a.cells / a.n.max(1),
                a.max_cells,
            );
        }
        let mut sites: Vec<_> = self.sites.iter().collect();
        sites.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for ((child, caller, line, col), n) in sites.into_iter().take(12) {
            eprintln!(
                "[profile] site {} <- {} @{line}:{col} analyses={n}",
                self.name(*child),
                self.name(*caller)
            );
        }
    }
}

/// The `n` largest entries of `m`, largest first, ties by key.
fn top_n<V: Ord + Copy>(m: &HashMap<&'static str, V>, n: usize) -> Vec<(&'static str, V)> {
    let mut v: Vec<_> = m.iter().map(|(k, v)| (*k, *v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.truncate(n);
    v
}

fn with(f: impl FnOnce(&mut Profile)) {
    PROFILE.with(|p| f(&mut p.borrow_mut()));
}

/// An open phase or analysis; closes when dropped.
#[must_use]
pub struct Guard(GuardKind);

enum GuardKind {
    Off,
    Phase,
    Analysis,
}

const OFF: Guard = Guard(GuardKind::Off);

impl Drop for Guard {
    fn drop(&mut self) {
        match self.0 {
            GuardKind::Off => {}
            GuardKind::Phase => with(|p| {
                p.switch();
                p.stack.pop();
            }),
            GuardKind::Analysis => with(|p| {
                p.switch();
                p.stack.pop();
                if let Some(o) = p.open.pop() {
                    let a = p.analyses.entry((o.component, o.kind)).or_default();
                    a.inclusive += o.start.elapsed();
                }
            }),
        }
    }
}

fn push(phase: &'static str, origin: Option<&'static str>) -> Guard {
    if !on() {
        return OFF;
    }
    with(|p| {
        p.switch();
        let inherited = p.stack.last().map_or("other", |f| f.origin);
        p.stack.push(Frame {
            phase,
            origin: origin.unwrap_or(inherited),
        });
    });
    Guard(GuardKind::Phase)
}

/// Charge the time until the guard drops to `phase`; the body runs under it
/// keep the origin of the phase around it.
pub fn phase(phase: &'static str) -> Guard {
    push(phase, None)
}

/// A fixpoint pass: charged like [`phase`], and the origin of the body runs
/// under it.
pub fn pass(phase: &'static str) -> Guard {
    push(phase, Some(phase))
}

/// One analysis of `component` (`kind`: root, child, probe or intra), handed
/// a heap of `cells` sites of which `closures` are closures.
pub fn analysis(
    component: ComponentId,
    name: &str,
    kind: &'static str,
    cells: u64,
    closures: u64,
) -> Guard {
    if !on() {
        return OFF;
    }
    with(|p| {
        p.switch();
        p.names.entry(component).or_insert_with(|| name.to_string());
        p.stack.push(Frame {
            phase: "fixpoint",
            origin: "fixpoint",
        });
        p.open.push(Open {
            component,
            kind,
            start: Instant::now(),
        });
        let a = p.analyses.entry((component, kind)).or_default();
        a.n += 1;
        a.cells += cells;
        a.max_cells = a.max_cells.max(cells);
        a.closures += closures;
        a.max_closures = a.max_closures.max(closures);
    });
    Guard(GuardKind::Analysis)
}

/// One run of a body at nesting `depth`: a pass's own body at 0, a closure
/// it runs below. A closure's run is charged to `closure-runs`.
pub fn body_run(depth: usize) -> Guard {
    if !on() {
        return OFF;
    }
    with(|p| {
        p.runs += 1;
        p.runs_by_depth[depth.min(5)] += 1;
        let origin = p.stack.last().map_or("other", |f| f.origin);
        *p.runs_by_origin.entry(origin).or_default() += 1;
        if let Some(o) = p.open.last() {
            let key = (o.component, o.kind);
            p.analyses.entry(key).or_default().runs += 1;
        }
    });
    match depth {
        0 => OFF,
        _ => phase("closure-runs"),
    }
}

/// The check of the rule `name`: one phase per rule.
pub fn rule(name: &str) -> Guard {
    if !on() {
        return OFF;
    }
    thread_local! {
        static NAMES: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
    }
    // A phase is named by a `&'static str`, and the rules are a fixed few.
    let phase_name = NAMES.with(|n| {
        *n.borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| Box::leak(format!("rule:{name}").into_boxed_str()))
    });
    phase(phase_name)
}

/// An escaped closure run by the component that owns it, or by another.
pub fn escaped_run(owned: bool) {
    if !on() {
        return;
    }
    with(|p| match owned {
        true => p.escaped_owned += 1,
        false => p.escaped_foreign += 1,
    });
}

/// `caller` analysed `child` for the element at `line:col`.
pub fn site(child: ComponentId, caller: ComponentId, line: u32, col: u32) {
    if !on() {
        return;
    }
    with(|p| *p.sites.entry((child, caller, line, col)).or_default() += 1);
}

/// Print the tables, at the end of a run.
pub fn report() {
    if !on() {
        return;
    }
    with(|p| {
        p.switch();
        p.report();
    });
}
