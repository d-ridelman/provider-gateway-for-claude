// Runs in <head> before the first paint (the CSP allows only own script files, no inline ones):
// the chosen theme is remembered in the webview, so a light window does not flash dark at start.
(() => {
  let pref = "auto";
  try {
    pref = localStorage.getItem("pgc-theme") || "auto";
  } catch {
    /* storage is not available: follow the system */
  }
  const light = pref === "light" || (pref === "auto" && matchMedia("(prefers-color-scheme: light)").matches);
  document.documentElement.dataset.theme = light ? "light" : "dark";
})();
