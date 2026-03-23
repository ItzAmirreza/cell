/**
 * Cell CLI bridge — calls `cell --json` commands and returns parsed data.
 */

import { $ } from "bun";

// Path to the cell binary (adjust if needed)
const CELL_BIN = process.env.CELL_BIN || "../target/debug/cell.exe";

export interface CellImage {
  name: string;
  created_at: string;
  layers: number;
}

export interface CellContainer {
  id: string;
  image: string;
  status: string;
  pid: number | null;
  created_at: string;
}

export interface CellInfo {
  platform: string;
  method: string;
  filesystem: string;
  process: string;
  network: string;
  resources: string;
}

export interface RunResult {
  container_id: string;
  image: string;
  exit_code: number;
  status: string;
}

export interface BuildResult {
  name: string;
  layers: number;
  status: string;
}

async function cellExec(args: string[]): Promise<string> {
  const proc = Bun.spawn([CELL_BIN, "--json", ...args], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const stdout = await new Response(proc.stdout).text();
  await proc.exited;
  return stdout.trim();
}

export async function listImages(): Promise<CellImage[]> {
  try {
    const out = await cellExec(["images"]);
    return JSON.parse(out);
  } catch {
    return [];
  }
}

export async function listContainers(): Promise<CellContainer[]> {
  try {
    const out = await cellExec(["ps"]);
    return JSON.parse(out);
  } catch {
    return [];
  }
}

export async function getInfo(): Promise<CellInfo | null> {
  try {
    const out = await cellExec(["info"]);
    return JSON.parse(out);
  } catch {
    return null;
  }
}

export async function pullImage(reference: string): Promise<BuildResult | null> {
  try {
    const out = await cellExec(["pull", reference]);
    return JSON.parse(out);
  } catch {
    return null;
  }
}

export async function buildImage(path: string): Promise<BuildResult | null> {
  try {
    const out = await cellExec(["build", path]);
    return JSON.parse(out);
  } catch {
    return null;
  }
}

export async function runContainer(
  image: string,
  command?: string
): Promise<RunResult | null> {
  try {
    const args = ["run", image];
    if (command) args.push(command);
    const out = await cellExec(args);
    // The output may contain container stdout before the JSON line
    const lines = out.split("\n");
    const jsonLine = lines[lines.length - 1];
    return JSON.parse(jsonLine);
  } catch {
    return null;
  }
}

export async function stopContainer(id: string): Promise<boolean> {
  try {
    await cellExec(["stop", id]);
    return true;
  } catch {
    return false;
  }
}

export async function removeContainer(id: string): Promise<boolean> {
  try {
    await cellExec(["rm", id]);
    return true;
  } catch {
    return false;
  }
}

export function formatTime(iso: string): string {
  try {
    const d = new Date(iso);
    const now = new Date();
    const diff = now.getTime() - d.getTime();
    const mins = Math.floor(diff / 60000);
    const hours = Math.floor(mins / 60);
    const days = Math.floor(hours / 24);

    if (days > 0) return `${days}d ago`;
    if (hours > 0) return `${hours}h ago`;
    if (mins > 0) return `${mins}m ago`;
    return "just now";
  } catch {
    return iso.slice(0, 19);
  }
}
