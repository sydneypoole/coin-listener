export type ThemeMode = 'light' | 'dark' | 'system';

export const THEME_MODE_STORAGE_KEY = 'coin-listener:theme-mode';

function systemPrefersDark() {
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function loadThemeMode(): ThemeMode {
  const value = localStorage.getItem(THEME_MODE_STORAGE_KEY);
  return value === 'light' || value === 'dark' || value === 'system' ? value : 'system';
}

export function resolveThemeMode(mode: ThemeMode): 'light' | 'dark' {
  if (mode === 'system') {
    return systemPrefersDark() ? 'dark' : 'light';
  }
  return mode;
}

export function applyThemeMode(mode: ThemeMode) {
  const resolvedMode = resolveThemeMode(mode);
  document.documentElement.dataset.theme = resolvedMode;
  if (resolvedMode === 'dark') {
    document.body.setAttribute('theme-mode', 'dark');
    return;
  }
  document.body.removeAttribute('theme-mode');
}

export function saveThemeMode(mode: ThemeMode) {
  localStorage.setItem(THEME_MODE_STORAGE_KEY, mode);
  applyThemeMode(mode);
}

export function subscribeSystemTheme(callback: () => void) {
  const media = window.matchMedia('(prefers-color-scheme: dark)');
  media.addEventListener('change', callback);
  return () => media.removeEventListener('change', callback);
}
