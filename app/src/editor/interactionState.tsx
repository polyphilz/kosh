import { createContext, useContext, type ReactNode } from "react";

const KoshEditorDisabledContext = createContext(false);

export function KoshEditorInteractionProvider({
  children,
  disabled,
}: {
  children: ReactNode;
  disabled: boolean;
}) {
  return (
    <KoshEditorDisabledContext.Provider value={disabled}>
      {children}
    </KoshEditorDisabledContext.Provider>
  );
}

export function useKoshEditorDisabled(): boolean {
  return useContext(KoshEditorDisabledContext);
}
