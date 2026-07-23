Base surface container — 1px border, 16px radius, no shadow (shadows live only on the outer shell). Compose everything on top of it.

```jsx
<Card><h4>Project</h4><p>…</p></Card>
<Card interactive onClick={open} pad={16}>Clickable row</Card>
```
