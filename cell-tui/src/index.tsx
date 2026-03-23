/**
 * Cell TUI — Terminal dashboard for the Cell container runtime.
 * Uses OpenTUI's imperative core API.
 */

import { createCliRenderer, Box, Text, type Node } from "@opentui/core";
import * as cell from "./cell";

// ── Theme ──────────────────────────────────────────────────

const t = {
  accent: "#58a6ff",
  green: "#3fb950",
  red: "#f85149",
  yellow: "#d29922",
  cyan: "#79c0ff",
  dim: "#8b949e",
  text: "#e6edf3",
  bold: "#ffffff",
  border: "#30363d",
  bg: "#0d1117",
  panelBg: "#161b22",
};

// ── State ──────────────────────────────────────────────────

let images: cell.CellImage[] = [];
let containers: cell.CellContainer[] = [];
let info: cell.CellInfo | null = null;
let tab = 0;
let row = 0;
let status = "Starting...";
const tabs = ["Images", "Containers", "Info"];

// ── Rendering helpers ──────────────────────────────────────

function header(): Node {
  return Box(
    { flexDirection: "row", width: "100%", paddingX: 1, gap: 2 },
    Text({ content: " cell", fg: t.accent, bold: true }),
    ...tabs.map((name, i) =>
      Text({
        content: ` ${name} `,
        fg: i === tab ? t.bold : t.dim,
        bg: i === tab ? t.accent : undefined,
        bold: i === tab,
      })
    ),
    Text({ content: `  ${status}`, fg: t.dim })
  );
}

function imagesPanel(): Node {
  const hdr = Box(
    { flexDirection: "row", width: "100%", paddingX: 1 },
    Text({ content: "NAME".padEnd(26), fg: t.dim, bold: true }),
    Text({ content: "LAYERS".padEnd(10), fg: t.dim, bold: true }),
    Text({ content: "CREATED", fg: t.dim, bold: true })
  );

  const rows = images.length === 0
    ? [Text({ content: "  No images found. Use cell build or cell pull.", fg: t.dim })]
    : images.map((img, i) => {
        const sel = i === row;
        return Box(
          { flexDirection: "row", width: "100%", paddingX: 1, bg: sel ? t.accent : undefined },
          Text({ content: img.name.slice(0, 24).padEnd(26), fg: sel ? t.bold : t.text, bold: sel }),
          Text({ content: String(img.layers).padEnd(10), fg: sel ? t.bold : t.cyan }),
          Text({ content: cell.formatTime(img.created_at), fg: sel ? t.bold : t.dim })
        );
      });

  return Box({ flexDirection: "column", width: "100%" }, hdr, ...rows);
}

function containersPanel(): Node {
  const hdr = Box(
    { flexDirection: "row", width: "100%", paddingX: 1 },
    Text({ content: "ID".padEnd(12), fg: t.dim, bold: true }),
    Text({ content: "IMAGE".padEnd(18), fg: t.dim, bold: true }),
    Text({ content: "STATUS".padEnd(12), fg: t.dim, bold: true }),
    Text({ content: "CREATED", fg: t.dim, bold: true })
  );

  const rows = containers.length === 0
    ? [Text({ content: "  No containers found.", fg: t.dim })]
    : containers.map((c, i) => {
        const sel = i === row;
        const sc = c.status === "Running" ? t.green : c.status === "Stopped" ? t.red : t.cyan;
        return Box(
          { flexDirection: "row", width: "100%", paddingX: 1, bg: sel ? t.accent : undefined },
          Text({ content: c.id.padEnd(12), fg: sel ? t.bold : t.text, bold: sel }),
          Text({ content: c.image.slice(0, 16).padEnd(18), fg: sel ? t.bold : t.text }),
          Text({ content: c.status.padEnd(12), fg: sel ? t.bold : sc }),
          Text({ content: cell.formatTime(c.created_at), fg: sel ? t.bold : t.dim })
        );
      });

  return Box({ flexDirection: "column", width: "100%" }, hdr, ...rows);
}

function infoPanel(): Node {
  if (!info) return Text({ content: "  Loading...", fg: t.dim });

  const line = (label: string, val: string, color: string) =>
    Box(
      { flexDirection: "row", paddingX: 1, gap: 1 },
      Text({ content: `${label}:`.padEnd(14), fg: t.dim }),
      Text({ content: val, fg: color, bold: true })
    );

  return Box(
    { flexDirection: "column" },
    line("Platform", info.platform, t.accent),
    line("Method", info.method, t.cyan),
    line("Filesystem", info.filesystem, t.green),
    line("Process", info.process, t.green),
    line("Network", info.network, t.green),
    line("Resources", info.resources, t.green),
    Text({ content: "", fg: t.bg }),
    Box(
      { paddingX: 1 },
      Text({ content: `${images.length} images, ${containers.length} containers`, fg: t.dim })
    )
  );
}

function footer(): Node {
  const keys = [
    ["Tab", "panel"], ["j/k", "nav"], ["r", "refresh"],
    ["d", "delete"], ["s", "stop"], ["q", "quit"],
  ];
  return Box(
    { flexDirection: "row", width: "100%", paddingX: 1, gap: 1 },
    ...keys.map(([k, a]) =>
      Box(
        { flexDirection: "row" },
        Text({ content: ` ${k} `, fg: t.bold, bg: t.border, bold: true }),
        Text({ content: ` ${a}`, fg: t.dim })
      )
    )
  );
}

// ── Full render ────────────────────────────────────────────

let rootNode: Node;

function render() {
  // Remove all children from root, then re-add
  const children = rootNode.getChildren?.() || [];
  for (const child of children) {
    rootNode.remove(child);
  }

  const content = tab === 0 ? imagesPanel() : tab === 1 ? containersPanel() : infoPanel();

  rootNode.add(
    Box(
      { flexDirection: "column", width: "100%", height: "100%", bg: t.bg },
      header(),
      Text({ content: "" }),
      Box(
        {
          flexDirection: "column",
          borderStyle: "rounded",
          borderColor: t.border,
          width: "100%",
          flexGrow: 1,
          paddingX: 1,
          paddingY: 1,
        },
        Text({ content: ` ${tabs[tab]}`, fg: t.accent, bold: true }),
        Text({ content: "" }),
        content
      ),
      Text({ content: "" }),
      footer()
    )
  );
}

// ── Data ───────────────────────────────────────────────────

async function refresh() {
  status = "Refreshing...";
  render();
  [images, containers, info] = await Promise.all([
    cell.listImages(),
    cell.listContainers(),
    cell.getInfo(),
  ]);
  status = `${images.length} images, ${containers.length} containers`;
  row = 0;
  render();
}

// ── Input ──────────────────────────────────────────────────

function maxRows(): number {
  return tab === 0 ? images.length : tab === 1 ? containers.length : 0;
}

async function onKey(key: string) {
  if (key === "q") process.exit(0);
  if (key === "tab") { tab = (tab + 1) % tabs.length; row = 0; render(); return; }
  if (key === "j" || key === "down") { if (row < maxRows() - 1) row++; render(); return; }
  if (key === "k" || key === "up") { if (row > 0) row--; render(); return; }
  if (key === "r") { await refresh(); return; }
  if (key === "d" && tab === 1 && containers[row]) {
    status = `Removing ${containers[row].id}...`;
    render();
    await cell.removeContainer(containers[row].id);
    await refresh();
    return;
  }
  if (key === "s" && tab === 1 && containers[row]?.status === "Running") {
    status = `Stopping ${containers[row].id}...`;
    render();
    await cell.stopContainer(containers[row].id);
    await refresh();
    return;
  }
}

// ── Main ───────────────────────────────────────────────────

async function main() {
  const renderer = await createCliRenderer({ exitOnCtrlC: true });
  rootNode = renderer.root;

  // Register keyboard input handler
  renderer.addInputHandler((event: any) => {
    if (event?.type === "key" && event?.key) {
      onKey(event.key);
    }
    return false; // don't consume — let other handlers run
  });

  render();
  await refresh();

  // Keep process alive
  await new Promise(() => {});
}

main().catch((e) => {
  console.error("Fatal:", e);
  process.exit(1);
});
