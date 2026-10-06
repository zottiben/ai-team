import { api, post } from "./api";
import type { PlanStatus } from "./plan-api";

export type PlanProvenance = {
  source_path: string;
  source_plan: string | null;
  imported_at: string;
};

export type PlanLibraryEntry = {
  plan_id: number;
  slug: string;
  title: string;
  status: PlanStatus;
  summary: string | null;
  project_id: number;
  project_slug: string;
  project_name: string;
  chat_id: number;
  chat_title: string;
  chat_archived: boolean;
  slices: number;
  done: number;
  open_questions: number;
  updated_at: string;
  last_activity: string | null;
  imported: PlanProvenance | null;
};

export type PlanLibraryProject = {
  id: number;
  slug: string;
  name: string;
  plans: number;
};

/** A chat an import may be approved into: idle, empty, and with no plan of its own. */
export type PlanImportTarget = {
  chat_id: number;
  project_id: number;
  project_slug: string;
  project_name: string;
  title: string;
  workspace_path: string;
  created_at: string;
};

export type DetachedPlan = {
  plan_id: number;
  slug: string;
  title: string;
  repo_key: string;
  why: string;
};

export type PlanLibrary = {
  entries: PlanLibraryEntry[];
  projects: PlanLibraryProject[];
  destinations: PlanImportTarget[];
  detached: DetachedPlan[];
};

export type PlanSource = {
  path: string;
  bytes: number;
  digest: string;
  schema_version: number;
  plans: number;
};

export type PlanImportedInto = {
  chat_id: number;
  project_slug: string | null;
  title: string;
  imported_at: string;
};

export type PlanSourcePlan = {
  id: number;
  repo_key: string;
  repo_name: string;
  slug: string;
  title: string;
  status: PlanStatus;
  summary: string | null;
  slices: number;
  done: number;
  open_questions: number;
  created_at: string;
  updated_at: string;
  already_imported: PlanImportedInto | null;
};

export type PlanSourceSurvey = { source: PlanSource; plans: PlanSourcePlan[] };

export type PlanImportCounts = {
  sections: number;
  slices: number;
  slice_deps: number;
  decisions: number;
  questions: number;
  gotchas: number;
  log: number;
  sources: number;
  handoffs: number;
  raw_bytes: number;
  file_imports: number;
  affinities: number;
  embeddings: number;
};

export type PlanImportEvidence = {
  key: string;
  title: string;
  status: PlanStatus;
  claimed_by: string | null;
  claimed_at: string | null;
  worktree_path: string | null;
  branch: string | null;
  base_branch: string | null;
  pr_url: string | null;
};

export type PlanImportPreview = {
  source: PlanSource;
  plan: PlanSourcePlan;
  /** Echoed back on approval; the server refuses an import of anything else. */
  fingerprint: string;
  counts: PlanImportCounts;
  preserved: string[];
  warnings: string[];
  evidence: PlanImportEvidence[];
  refusal: string | null;
};

export type PlanImported = {
  chat_id: number;
  project_id: number;
  project_slug: string;
  plan_id: number;
  slug: string;
  title: string;
  revision: number;
  counts: PlanImportCounts;
  warnings: string[];
  source: PlanSource;
};

export function planLibrary(filter: {
  project?: string | null;
  status?: PlanStatus[];
}): Promise<PlanLibrary> {
  const query = new URLSearchParams();
  if (filter.project) query.set("project", filter.project);
  if (filter.status?.length) query.set("status", filter.status.join(","));
  const search = query.toString();
  return api<PlanLibrary>(`/plan-library${search ? `?${search}` : ""}`);
}

/** Read a database the operator named. Nothing here defaults to a path. */
export function readPlanSource(path: string): Promise<PlanSourceSurvey> {
  return post("/plan-library/source", { path });
}

export function previewPlanImport(
  path: string,
  planId: number,
): Promise<PlanImportPreview> {
  return post("/plan-library/preview", { path, plan_id: planId });
}

export function importPlan(request: {
  path: string;
  plan_id: number;
  chat_id: number;
  fingerprint: string;
}): Promise<PlanImported> {
  return post("/plan-library/import", request);
}
