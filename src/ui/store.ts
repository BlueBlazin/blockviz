import { useSyncExternalStore } from 'react';

/** A value that changes, read by the components that show it, so that only they re-render. */
export interface Source<T> {
  get(): T;
  subscribe(listener: () => void): () => void;
}

/** A value set from outside React, such as the block under the pointer. */
export class Store<T> implements Source<T> {
  private listeners = new Set<() => void>();

  constructor(private value: T) {}

  get = () => this.value;

  set(value: T) {
    if (Object.is(value, this.value)) return;
    this.value = value;
    for (const l of this.listeners) l();
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
}

/** `source` seen through `f`. */
export const derive = <T, U>(source: Source<T>, f: (value: T) => U): Source<U> => ({ get: () => f(source.get()), subscribe: source.subscribe });

/** What `select` picks from `source`: the component re-renders only when that changes. */
export const useSource = <T, U>(source: Source<T>, select: (value: T) => U): U => useSyncExternalStore(source.subscribe, () => select(source.get()));
