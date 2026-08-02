import { listen } from "@tauri-apps/api/event";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useBackend } from "../backend/context";
import type {
  KeyboardBinding,
  SetShortcutSettingsInput,
  ShortcutSettingsSnapshot,
} from "../backend/contracts";
import { TauriEvent } from "../tauriProtocol";

interface ShortcutSettingsContextValue {
  error: string | null;
  loading: boolean;
  settings: ShortcutSettingsSnapshot | null;
  updateAutomaticChecks: (enabled: boolean) => Promise<void>;
  update: (input: SetShortcutSettingsInput) => Promise<void>;
}

const ShortcutSettingsContext = createContext<ShortcutSettingsContextValue | null>(null);

export function ShortcutSettingsProvider({ children }: { children: ReactNode }) {
  const backend = useBackend();
  const [settings, setSettings] = useState<ShortcutSettingsSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setSettings(await backend.loadShortcutSettings());
      setError(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, [backend]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<ShortcutSettingsSnapshot>(TauriEvent.ShortcutSettingsChanged, (event) => {
      if (active) {
        setSettings(cloneSettings(event.payload));
        setError(null);
      }
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const update = useCallback(
    async (input: SetShortcutSettingsInput) => {
      setLoading(true);
      setError(null);
      try {
        setSettings(await backend.setShortcutSettings(input));
      } catch (reason) {
        setError(errorMessage(reason));
        try {
          setSettings(await backend.loadShortcutSettings());
        } catch {
          // Preserve the original mutation error.
        }
        throw reason;
      } finally {
        setLoading(false);
      }
    },
    [backend],
  );

  const updateAutomaticChecks = useCallback(
    async (enabled: boolean) => {
      if (!settings) return;
      setLoading(true);
      setError(null);
      try {
        setSettings(
          await backend.setAutomaticUpdateChecks({
            enabled,
            expectedRevision: settings.revision,
          }),
        );
      } catch (reason) {
        setError(errorMessage(reason));
        try {
          setSettings(await backend.loadShortcutSettings());
        } catch {
          // Preserve the original mutation error.
        }
        throw reason;
      } finally {
        setLoading(false);
      }
    },
    [backend, settings],
  );

  const value = useMemo(
    () => ({ error, loading, settings, update, updateAutomaticChecks }),
    [error, loading, settings, update, updateAutomaticChecks],
  );
  return (
    <ShortcutSettingsContext.Provider value={value}>{children}</ShortcutSettingsContext.Provider>
  );
}

export function useShortcutSettings(): ShortcutSettingsContextValue {
  const value = useContext(ShortcutSettingsContext);
  if (!value) {
    throw new Error("useShortcutSettings must be used inside ShortcutSettingsProvider");
  }
  return value;
}

export function bindingFor(
  bindings: readonly KeyboardBinding[],
  command: KeyboardBinding["command"],
): KeyboardBinding | undefined {
  return bindings.find((binding) => binding.command === command);
}

function cloneSettings(settings: ShortcutSettingsSnapshot): ShortcutSettingsSnapshot {
  return {
    ...settings,
    keyboardBindings: settings.keyboardBindings.map((binding) => ({ ...binding })),
    shortcutErrors: [...settings.shortcutErrors],
  };
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
