// A cached document owns live resources until a final, non-persisted exit.
export function installMapLifecycle(target, { dispose, resume }) {
  let disposed = false;
  function finalize() {
    if (disposed) return;
    disposed = true;
    target.removeEventListener('pagehide', hide);
    target.removeEventListener('pageshow', show);
    dispose();
  }
  function hide(event) {
    if (!event.persisted) finalize();
  }
  function show(event) {
    if (event.persisted && !disposed) resume();
  }
  target.addEventListener('pagehide', hide);
  target.addEventListener('pageshow', show);
  return finalize;
}
