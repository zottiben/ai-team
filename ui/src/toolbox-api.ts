import { api, post } from "./api";

export type ToolboxScan = {
  root: string; catalogue_revision: string; problem: string | null;
  worktree_problem: string | null;
  worktrees: { path: string; branch: string | null; different: string[]; problem: string | null }[];
  survey: {
    state: string; harnesses: string[];
    inventory: { agents_md: boolean; claude_md: boolean; pi: { adapter_servers: string[] } };
    items: { name: string; kind: string; origin: { state: string } }[];
    findings: { what: string; advice: string | null; repairable: boolean }[];
    recommendation: { detected: string[]; hooks: string[]; mcp: string[]; skills: string[]; rules: string[]; notes: string[] };
  } | null;
};
export type SetupNode =
  | { kind: "missing" }
  | { kind: "file"; contents: number[]; mode: number }
  | { kind: "directory"; entries: Record<string, SetupNode>; mode: number }
  | { kind: "symlink"; target: string };
export type SetupPreview = {
  id: number; project_id: number; root: string; catalogue_revision: string;
  effects: { path: string; summary: string; before: SetupNode; after: SetupNode }[];
  warnings: string[]; state: "preview" | "applying" | "applied" | "refused" | "partial";
  outcome: { applied: string[]; problem: string | null; uncertain: boolean } | null;
};
export type SetupSelection = { operation: "repair" } | {
  operation: "install"; harnesses: string[]; hooks: string[]; mcp: string[]; skills: string[]; scaffold: boolean;
};
export type SetupRecord = Pick<SetupPreview, "id" | "root" | "state" | "outcome">;
export const setupHistory = (project: number): Promise<SetupRecord[]> => api(`/projects/${project}/toolbox/history`);
export const savedSetup = (project: number, id: number): Promise<SetupPreview> => api(`/projects/${project}/toolbox/previews/${id}`);
export const scanToolbox = (project: number): Promise<ToolboxScan[]> => api(`/projects/${project}/toolbox`);
export const previewSetup = (project: number, root: string, selection: SetupSelection): Promise<SetupPreview> => post(`/projects/${project}/toolbox/preview`, { root, selection });
export const approveSetup = (project: number, preview: number): Promise<SetupPreview> => post(`/projects/${project}/toolbox/previews/${preview}/apply`, {});
