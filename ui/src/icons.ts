/** Stroke icons on a 24px grid, one path string each. */
const rect = (x: number, y: number, w: number, h: number, r = 0) =>
  `M${x + r} ${y}h${w - 2 * r}a${r} ${r} 0 0 1 ${r} ${r}v${h - 2 * r}a${r} ${r} 0 0 1 ${-r} ${r}h${-(w - 2 * r)}a${r} ${r} 0 0 1 ${-r} ${-r}v${-(h - 2 * r)}a${r} ${r} 0 0 1 ${r} ${-r}z`;
const circle = (cx: number, cy: number, r: number) =>
  `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0`;

export const ICONS = {
  logo: `M12 2.5l8.5 5v9l-8.5 5-8.5-5v-9z ${circle(12, 12, 2.6)} M12 9.4V2.5M14.3 13.3l6.2 3.7M9.7 13.3L3.5 17`,
  plus: "M12 5v14M5 12h14",
  close: "M6 6l12 12M18 6L6 18",
  check: "M5 12.5l4.5 4.5L19 7.5",
  alert: "M12 3l10 18H2z M12 10v5M12 18h.01",
  info: `${circle(12, 12, 9)} M12 11v5M12 8h.01`,
  box: "M12 3l8 4.5v9L12 21l-8-4.5v-9z M12 12l8-4.5M12 12L4 7.5M12 12v9",
  key: `${circle(8, 15, 4)} M11 12l9-9M16 7l3 3`,
  user: `${circle(12, 8, 4)} M4 21c0-4 3.6-7 8-7s8 3 8 7`,
  logout: "M9 4H5a2 2 0 00-2 2v12a2 2 0 002 2h4M16 8l4 4-4 4M20 12H9",
  chat: "M4 5h16v11H9l-5 4z",
  folder: "M3 7a2 2 0 012-2h4l2 2.5h8a2 2 0 012 2V18a2 2 0 01-2 2H5a2 2 0 01-2-2z",
  file: "M14 3H7a2 2 0 00-2 2v14a2 2 0 002 2h10a2 2 0 002-2V8z M14 3v5h5",
  terminal: `${rect(3, 4, 18, 16, 2)} M7 9l3 3-3 3M12 15h5`,
  play: "M7 4.5v15l12-7.5z",
  stop: rect(6, 6, 12, 12, 2),
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13",
  refresh: "M20 11a8 8 0 00-14.9-3M4 4v4h4M4 13a8 8 0 0014.9 3M20 20v-4h-4",
  send: "M5 12h14M13 6l6 6-6 6",
  up: "M12 19V5M6 11l6-6 6 6",
  chevron: "M9 6l6 6-6 6",
  lock: `${rect(5, 11, 14, 10, 2)} M8 11V8a4 4 0 018 0v3`,
} as const;

export type IconName = keyof typeof ICONS;
