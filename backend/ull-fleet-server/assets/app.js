// Keeps pages current without reloads: every 10 s, re-fetch the page and swap
// the elements marked data-live. Also asks before destructive form posts.
(() => {
  "use strict";

  const url = new URL(location.href);
  if (url.searchParams.has("notice") || url.searchParams.has("error")) {
    url.searchParams.delete("notice");
    url.searchParams.delete("error");
    history.replaceState(null, "", url);
  }

  document.addEventListener("submit", (event) => {
    const message = event.target.dataset.confirm;
    if (message && !confirm(message)) event.preventDefault();
  });

  if (!document.querySelector("[data-live]")) return;

  let busy = false;
  async function refresh() {
    const active = document.activeElement;
    if (busy || document.hidden || (active && active.closest("form"))) return;
    busy = true;
    try {
      const response = await fetch(location.href, { credentials: "same-origin" });
      if (!response.ok || response.redirected) return;
      const next = new DOMParser().parseFromString(await response.text(), "text/html");
      for (const element of document.querySelectorAll("[data-live][id]")) {
        const fresh = next.getElementById(element.id);
        if (fresh && fresh.innerHTML !== element.innerHTML) element.replaceWith(fresh);
      }
    } catch {
      // Offline or server restarting; try again next tick.
    } finally {
      busy = false;
    }
  }

  setInterval(refresh, 10000);
  document.addEventListener("visibilitychange", refresh);
})();
