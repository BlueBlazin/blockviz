import { useCallback, useSyncExternalStore } from 'react';

/** Phones and narrow windows: the bars stack and the details panel is a bottom sheet. */
export const NARROW = '(max-width: 760px)';
/** A finger, not a mouse, is the main pointer. */
export const TOUCH = '(pointer: coarse)';

/** Whether a media query matches, kept up to date. */
export function useMedia(query: string): boolean {
  const subscribe = useCallback((changed: () => void) => {
    const m = window.matchMedia(query);
    m.addEventListener('change', changed);
    return () => m.removeEventListener('change', changed);
  }, [query]);
  return useSyncExternalStore(subscribe, () => window.matchMedia(query).matches);
}

const onResize = (changed: () => void) => {
  window.addEventListener('resize', changed);
  return () => window.removeEventListener('resize', changed);
};

/** The window's width, kept up to date. */
export function useWindowWidth(): number {
  return useSyncExternalStore(onResize, () => window.innerWidth);
}
