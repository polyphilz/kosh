import { createContext, useContext } from "react";
import type { TidbitRecord } from "../backend/contracts";

export type AnnounceDeletedNote = (note: TidbitRecord) => void;

export const NoteDeletionContext = createContext<AnnounceDeletedNote | null>(null);

export function useNoteDeletion(): AnnounceDeletedNote {
  const announce = useContext(NoteDeletionContext);
  if (!announce) throw new Error("NoteDeletionContext is missing");
  return announce;
}
