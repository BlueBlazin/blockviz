import type { CompareInfo, InputFile, LeftOut } from './types';

/** A file as downloaded, with the hash GitHub names its content by. */
export interface SavedFile extends InputFile { sha?: string }

/** A downloaded repository, kept so it opens again without downloading. A
 * comparison (a pull request) keeps both versions: `files` is the head. */
export interface SavedRepo {
  key: string;
  name: string;
  description?: string;
  source: string;
  files: SavedFile[];
  base?: SavedFile[];
  compare?: CompareInfo;
  /** What the limits left out of the files (the head's, of a comparison). */
  leftOut?: LeftOut;
  /** When it was downloaded or last checked against GitHub (ms since the epoch). */
  saved: number;
}

export interface RepoCache {
  get(key: string): Promise<SavedRepo | undefined>;
  put(repo: SavedRepo): Promise<void>;
}

/** How many repositories, and how many analyzed scenes (a comparison is two), are kept. */
const KEEP = { repos: 5, scenes: 4 };
type Store = keyof typeof KEEP;

let db: Promise<IDBDatabase | null> | undefined;

/** This browser's database (IndexedDB), or null without storage (a private
 * window, blocked site data). Everything kept is optional: nothing here fails. */
function open(): Promise<IDBDatabase | null> {
  return (db ??= new Promise(resolve => {
    try {
      const req = indexedDB.open('blockviz', 2);
      req.onupgradeneeded = () => {
        for (const name of Object.keys(KEEP)) {
          if (!req.result.objectStoreNames.contains(name)) req.result.createObjectStore(name, { keyPath: 'key' }).createIndex('saved', 'saved');
        }
      };
      // A newer version of the page, open in another tab, may upgrade it.
      req.onsuccess = () => { req.result.onversionchange = () => req.result.close(); resolve(req.result); };
      req.onerror = () => resolve(null);
      req.onblocked = () => resolve(null);
    } catch {
      resolve(null);
    }
  }));
}

async function run<T>(name: Store, mode: IDBTransactionMode, work: (store: IDBObjectStore) => IDBRequest<T> | void): Promise<T | undefined> {
  const d = await open();
  if (!d) return undefined;
  return new Promise(resolve => {
    try {
      const tx = d.transaction(name, mode);
      const req = work(tx.objectStore(name));
      tx.oncomplete = () => resolve(req ? req.result : undefined);
      tx.onerror = () => resolve(undefined);
      tx.onabort = () => resolve(undefined);
    } catch {
      resolve(undefined);
    }
  });
}

/** Keeps `value`, forgetting the longest kept beyond the few of its store (a
 * repository in use is saved again when checked, daily). */
async function save<T extends { key: string; saved: number }>(name: Store, value: T): Promise<void> {
  await run(name, 'readwrite', store => {
    store.put(value);
    const keys = store.index('saved').getAllKeys();
    keys.onsuccess = () => { for (const k of keys.result.slice(0, -KEEP[name])) store.delete(k); };
  });
}

/** Repositories kept in this browser. */
export function browserCache(): RepoCache {
  return {
    get: key => run<SavedRepo>('repos', 'readonly', s => s.get(key)),
    put: repo => save('repos', repo),
  };
}

/** Scenes the analyzer made (its reply's bytes), named by everything they
 * were made from: see analysis.worker.ts. */
export const scenes = {
  get: async (key: string) => (await run<{ scene: Uint8Array }>('scenes', 'readonly', s => s.get(key)))?.scene,
  put: (key: string, scene: Uint8Array) => save('scenes', { key, scene, saved: Date.now() }),
};

/** Repositories kept in memory, for tests. */
export function memoryCache(): RepoCache & { repos: Map<string, SavedRepo> } {
  const repos = new Map<string, SavedRepo>();
  return {
    repos,
    async get(key) { return repos.get(key); },
    async put(repo) { repos.set(repo.key, repo); },
  };
}
