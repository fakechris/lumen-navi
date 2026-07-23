Atelier ⇄ Vault theme switch. Sets `data-theme` on `<html>` and persists to localStorage. Uncontrolled by default — drop it in a top bar / sidebar footer and it just works.

```jsx
<ThemeToggle />
<ThemeToggle storageKey="ovp-theme" />   {/* OVP2 portal */}
```

For no-flash theming, set `data-theme` on `<html>` from a tiny boot script before first paint (see README → Theming).
