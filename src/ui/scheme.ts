import { useSyncExternalStore } from 'react';

/**
 * Light or dark: the reader's choice, remembered, or else the system's
 * setting, followed as it changes. The page's colors follow the root's
 * `color-scheme` (styles.css writes each as `light-dark()`), which a choice
 * sets through its `data-scheme`; index.html sets that from the remembered
 * choice before the page is first drawn.
 */
const KEY = 'blockviz.scheme';
const system = window.matchMedia('(prefers-color-scheme: dark)');
const changed = new Set<() => void>();

let choice = ((): 'light' | 'dark' | null => {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'light' || v === 'dark' ? v : null;
  } catch {
    return null;
  }
})();

const dark = () => (choice ? choice === 'dark' : system.matches);

function subscribe(listener: () => void) {
  changed.add(listener);
  system.addEventListener('change', listener);
  return () => {
    changed.delete(listener);
    system.removeEventListener('change', listener);
  };
}

/** Whether the page is dark, kept up to date. */
export function useDark(): boolean {
  return useSyncExternalStore(subscribe, dark);
}

/** Switches to the other scheme. Choosing the system's own forgets the choice, so the page follows the system again. */
export function toggleScheme() {
  const next = dark() ? 'light' : 'dark';
  choice = next === (system.matches ? 'dark' : 'light') ? null : next;
  try {
    if (choice) localStorage.setItem(KEY, choice);
    else localStorage.removeItem(KEY);
  } catch { /* remembered for this visit only */ }
  if (choice) document.documentElement.dataset.scheme = choice;
  else delete document.documentElement.dataset.scheme;
  for (const listener of changed) listener();
}
