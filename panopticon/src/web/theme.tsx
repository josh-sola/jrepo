import { createContext, use, useEffect, useState, type ReactNode } from 'react';

type Theme = 'light' | 'dark';

const STORAGE_KEY = 'panopticon-theme';

const ThemeContext = createContext<{
  theme: Theme;
  setTheme: (theme: Theme) => void;
} | null>(null);

function readStoredTheme(): Theme {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === 'light' || stored === 'dark') return stored;
  } catch {
    // localStorage can throw in private browsing; fall back to light.
  }
  return 'light';
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(readStoredTheme);

  useEffect(() => {
    document.documentElement.classList.toggle('dark', theme === 'dark');
    try {
      localStorage.setItem(STORAGE_KEY, theme);
    } catch {
      // Nothing to persist to if storage is unavailable.
    }
  }, [theme]);

  function setTheme(next: Theme) {
    setThemeState(next);
  }

  return <ThemeContext value={{ theme, setTheme }}>{children}</ThemeContext>;
}

export function useTheme() {
  const context = use(ThemeContext);
  if (!context) throw new Error('useTheme must be used inside a ThemeProvider');
  return context;
}
