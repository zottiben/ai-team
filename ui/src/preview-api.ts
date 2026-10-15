import { api, type FileDiff } from "./api";

export type PreviewFamily = "checkout" | "delivery";
export type PreviewEvidence = {
  basis: string;
  files_total: number | null;
  omitted_files: number;
  files: { area: string; file: FileDiff; revision: string | null; reason: string | null; hunks_omitted: boolean }[];
  note: string | null;
};
export const previewEvidence = (chat: number, family: PreviewFamily, operation: number) =>
  api<PreviewEvidence | null>(`/chats/${chat}/${family}/preview/${operation}`);
