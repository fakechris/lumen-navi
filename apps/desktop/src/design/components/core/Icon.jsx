import React from "react";

/**
 * The Lumen icon set — outline glyphs lifted verbatim from lumen-cut's
 * hand-drawn Icons.tsx: 24×24 viewBox, 1.8px stroke, round caps/joins,
 * currentColor. This IS the family's icon style. For glyphs beyond this
 * set, use Lucide (lucide.dev) — same stroke weight and rounded outline.
 */
const P = { fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeLinejoin: "round", strokeWidth: 1.8 };

const PATHS = {
  folder: <><path {...P} d="M3.75 6.75h5l2 2h9.5v8.5a2 2 0 0 1-2 2H5.75a2 2 0 0 1-2-2v-10.5Z" /><path {...P} d="M3.75 9h16.5" /></>,
  transcript: <><path {...P} d="M6.25 3.75h8l3.5 3.5v13H6.25a2 2 0 0 1-2-2V5.75a2 2 0 0 1 2-2Z" /><path {...P} d="M14.25 3.75v4h3.5M8 12h6.5M8 15.5h5M8 8.5h2.5" /></>,
  settings: <><circle {...P} cx="12" cy="12" r="3" /><path {...P} d="m19 13.7 1.1 1.8-2.1 2.1-1.8-1.1a7.7 7.7 0 0 1-2 .8l-.5 2.1h-3.4l-.5-2.1a7.7 7.7 0 0 1-2-.8L6 17.6l-2.1-2.1L5 13.7a7.8 7.8 0 0 1 0-3.4L3.9 8.5 6 6.4l1.8 1.1a7.7 7.7 0 0 1 2-.8l.5-2.1h3.4l.5 2.1a7.7 7.7 0 0 1 2 .8L18 6.4l2.1 2.1-1.1 1.8a7.8 7.8 0 0 1 0 3.4Z" /></>,
  upload: <path {...P} d="M12 15.5V4.5M8 8.5l4-4 4 4M5 13.5v4.75a1.75 1.75 0 0 0 1.75 1.75h10.5A1.75 1.75 0 0 0 19 18.25V13.5" />,
  link: <path {...P} d="m9.5 14.5 5-5M8 16l-1 1a3.5 3.5 0 0 1-5-5l3-3a3.5 3.5 0 0 1 5 0M16 8l1-1a3.5 3.5 0 1 1 5 5l-3 3a3.5 3.5 0 0 1-5 0" />,
  microphone: <><rect {...P} height="10" rx="3" width="6" x="9" y="3" /><path {...P} d="M6.5 11.5a5.5 5.5 0 0 0 11 0M12 17v4M9 21h6" /></>,
  play: <><circle {...P} cx="12" cy="12" r="9" /><path {...P} d="m10 8.5 5 3.5-5 3.5v-7Z" /></>,
  chevronRight: <path {...P} d="m9 5 7 7-7 7" />,
  chevronDown: <path {...P} d="m5 9 7 7 7-7" />,
  search: <><circle {...P} cx="10.5" cy="10.5" r="6.5" /><path {...P} d="m15.5 15.5 4.25 4.25" /></>,
  star: <path {...P} d="m12 3.5 2.6 5.27 5.82.85-4.21 4.1.99 5.8L12 16.78l-5.2 2.74.99-5.8-4.21-4.1 5.82-.85L12 3.5Z" />,
  sun: <><circle {...P} cx="12" cy="12" r="3.5" /><path {...P} d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4" /></>,
  moon: <path {...P} d="M20 15.2A8.6 8.6 0 0 1 8.8 4a8.7 8.7 0 1 0 11.2 11.2Z" />,
  check: <path {...P} d="m5 12.5 4.3 4.2L19 7" />,
  alert: <><path {...P} d="M12 3 2.8 19h18.4L12 3Z" /><path {...P} d="M12 9v4M12 16.5v.1" /></>,
  translate: <><path {...P} d="M4 6h9M8.5 4v2M10.5 6c-.6 4-3 6.8-6.5 8.5M6 10c.9 2 2.8 3.5 5 4.5" /><path {...P} d="M13 20l3.5-8 3.5 8M14.3 17h4.4" /></>,
  layers: <><path {...P} d="M12 3 3 8l9 5 9-5-9-5Z" /><path {...P} d="m3 12 9 5 9-5M3 16l9 5 9-5" /></>,
  close: <path {...P} d="M6 6l12 12M18 6 6 18" />,
};

export function Icon({ name, size = 20, ...props }) {
  const glyph = PATHS[name];
  return (
    <svg aria-hidden="true" fill="none" width={size} height={size} viewBox="0 0 24 24" {...props}>
      {glyph}
    </svg>
  );
}

export const iconNames = Object.keys(PATHS);
