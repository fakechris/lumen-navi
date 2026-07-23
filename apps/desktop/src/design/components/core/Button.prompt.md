Terracotta-filled primary action button for the Lumen family; use for the main action in any view, with `secondary`/`ghost` for lower-priority actions and `danger` for destructive ones.

```jsx
<Button variant="primary" icon="microphone">Start dictation</Button>
<Button variant="secondary">Cancel</Button>
<Button variant="ghost" size="sm">Skip</Button>
<Button variant="danger" icon="close">Delete project</Button>
<Button selected>Bold</Button>
```

Variants: `primary` (accent fill) · `secondary` (bordered surface) · `ghost` (transparent) · `danger`. Sizes `sm`/`md`/`lg`. `selected` gives an accent-tinted toggle state; `fullWidth` stretches to the container. Hover changes background/opacity only — never position (quiet-utility rule).
