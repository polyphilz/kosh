import { createContext, useContext, type ReactNode } from "react";
import type { Backend } from "./contracts";

const BackendContext = createContext<Backend | null>(null);

interface BackendProviderProps {
  backend: Backend;
  children: ReactNode;
}

export function BackendProvider({ backend, children }: BackendProviderProps) {
  return <BackendContext.Provider value={backend}>{children}</BackendContext.Provider>;
}

export function useBackend(): Backend {
  const backend = useContext(BackendContext);
  if (!backend) {
    throw new Error("BackendProvider is missing");
  }
  return backend;
}
