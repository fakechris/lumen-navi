Icon-only square button for toolbars, theme toggles, and row affordances; transparent until hover, which reveals a surface fill.

```jsx
<IconButton icon="search" label="Search" />
<IconButton icon="star" label="Favorite" active />
<IconButton icon="settings" label="Settings" size="lg" />
```

Always pass `label` (used as aria-label + tooltip). `active` gives the accent-tinted pressed state.
