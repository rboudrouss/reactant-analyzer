// Local utility — body is short, statement-level inlining swaps its CFG
// in place of the call site.

function bump(setter) {
  setter(1);
}
