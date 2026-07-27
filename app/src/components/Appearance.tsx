import { createContext, useContext, useLayoutEffect, useState, type ReactNode } from "react";

export type Appearance = "SYSTEM" | "LIGHT" | "DARK";

interface AppearanceContextValue {
  appearance: Appearance;
  setAppearance: (appearance: Appearance) => void;
}

interface AppearanceProviderProps {
  children: ReactNode;
}

const appearanceStorageKey = "kosh.appearance";
const appearances = new Set<Appearance>(["SYSTEM", "LIGHT", "DARK"]);
const AppearanceContext = createContext<AppearanceContextValue | null>(null);

function loadAppearance(): Appearance {
  try {
    const stored = window.localStorage.getItem(appearanceStorageKey);
    return stored && appearances.has(stored as Appearance) ? (stored as Appearance) : "SYSTEM";
  } catch {
    return "SYSTEM";
  }
}

export function AppearanceProvider({ children }: AppearanceProviderProps) {
  const [appearance, setAppearance] = useState<Appearance>(loadAppearance);

  useLayoutEffect(() => {
    document.documentElement.dataset.appearance = appearance;
    try {
      window.localStorage.setItem(appearanceStorageKey, appearance);
    } catch {
      // The selected palette still applies for this session when storage is unavailable.
    }
  }, [appearance]);

  return (
    <AppearanceContext.Provider value={{ appearance, setAppearance }}>
      {children}
    </AppearanceContext.Provider>
  );
}

export function useAppearance(): AppearanceContextValue {
  const context = useContext(AppearanceContext);
  if (!context) throw new Error("useAppearance must be used within AppearanceProvider");
  return context;
}
