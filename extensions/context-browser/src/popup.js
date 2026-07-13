const api = globalThis.browser ?? globalThis.chrome;
const site = document.getElementById("site");
const grant = document.getElementById("grant");
const status = document.getElementById("status");

const [tab] = await api.tabs.query({ active: true, lastFocusedWindow: true });
let pattern = null;
try {
  const url = new URL(tab.url);
  if (url.protocol === "http:" || url.protocol === "https:") {
    pattern = `${url.protocol}//${url.host}/*`;
  }
} catch {
  pattern = null;
}

if (!pattern) {
  site.textContent = "This browser page cannot be captured. AX and OCR fallbacks remain available.";
} else {
  site.textContent = pattern;
  const allowed = await api.permissions.contains({ origins: [pattern] });
  grant.disabled = allowed;
  grant.textContent = allowed ? "Site allowed" : "Allow this site";
  status.textContent = allowed ? "Access is limited to this origin and can be revoked in browser settings." : "No page content is read until you approve.";
}

grant.addEventListener("click", async () => {
  const allowed = await api.permissions.request({ origins: [pattern] });
  grant.disabled = allowed;
  grant.textContent = allowed ? "Site allowed" : "Permission not granted";
  status.textContent = allowed ? "This origin is now available to local context capture." : "The site remains unavailable.";
});
