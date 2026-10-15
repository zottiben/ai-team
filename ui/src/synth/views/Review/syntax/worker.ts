import { highlight } from "./highlight";
import type { SyntaxRequest, SyntaxResponse } from "./types";

self.onmessage = async (event: MessageEvent<SyntaxRequest>) => {
  const { id, input } = event.data;
  let result;
  try { result = await highlight(input); }
  catch { result = { state: "unavailable" as const, lines: {} }; }
  const response: SyntaxResponse = { id, result };
  self.postMessage(response);
};
