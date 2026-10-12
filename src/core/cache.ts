import type { InputFile } from './types';

/** A file as downloaded, with the hash GitHub names its content by. */
export interface SavedFile extends InputFile { sha?: string }

/** A downloaded repository, kept so it opens again without downloading. */
export interface SavedRepo {
  key: string;
  name: string;
  description?: string;
  source: string;
  files: SavedFile[];
  /** When it was downloaded or last checked against GitHub (ms since the epoch). */
  saved: number;
}

export interface RepoCache {
  get(key: string): Promise<SavedRepo | undefined>;
  put(repo: SavedRepo): Promise<void>;
}

/** How many repositories are kept. */
const KEEP = 5;

/** Repositories kept in this browser (IndexedDB). Never fails: without
 * storage (a private window, blocked site data) nothing is kept. */
export function browserCache(): RepoCache {
  let db: Promise<IDBDatabase | null> | null = null;
  const open = () => (db ??= new Promise<IDBDatabase | null>(resolve => {
    try {
      const req = indexedDB.open('blockviz', 1);
      req.onupgradeneeded = () => req.result.createObjectStore('repos', { keyPath: 'key' }).createIndex('saved', 'saved');
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
      req.onblocked = () => resolve(null);
    } catch {
      resolve(null);
    }
  }));
  const run = async <T>(mode: IDBTransactionMode, work: (store: IDBObjectStore) => IDBRequest<T> | void): Promise<T | undefined> => {
    const d = await open();
    if (!d) return undefined;
    return new Promise(resolve => {
      try {
        const tx = d.transaction('repos', mode);
        const req = work(tx.objectStore('repos'));
        tx.oncomplete = () => resolve(req ? req.result : undefined);
        tx.onerror = () => resolve(undefined);
        tx.onabort = () => resolve(undefined);
      } catch {
        resolve(undefined);
      }
    });
  };
  return {
    get: key => run<SavedRepo>('readonly', s => s.get(key)),
    async put(repo) {
      await run('readwrite', s => { s.put(repo); });
      // Forget the longest unchecked beyond the few kept (one in use is checked daily).
      const keys = await run<IDBValidKey[]>('readonly', s => s.index('saved').getAllKeys());
      const old = (keys ?? []).slice(0, -KEEP);
      if (old.length) await run('readwrite', s => { for (const k of old) s.delete(k); });
    },
  };
}

/** Repositories kept in memory, for tests. */
export function memoryCache(): RepoCache & { repos: Map<string, SavedRepo> } {
  const repos = new Map<string, SavedRepo>();
  return {
    repos,
    async get(key) { return repos.get(key); },
    async put(repo) { repos.set(repo.key, repo); },
  };
}
