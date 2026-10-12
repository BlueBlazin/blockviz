import { FileCode, FolderOpen, Link2, Sparkles, X } from 'lucide-react';
import { guessFilename } from '../core/loaders';
import { useEffect, useRef, useState } from 'react';
import { TOUCH, useMedia } from './media';

export type OpenRequest =
  | { kind: 'example' }
  | { kind: 'github'; url: string }
  | { kind: 'folder'; files: FileList }
  | { kind: 'paste'; filename: string; text: string };

export function OpenDialog({ onOpen, onClose, canClose, lastUrl }: { onOpen: (r: OpenRequest) => void; onClose: () => void; canClose: boolean; lastUrl: string }) {
  const [url, setUrl] = useState(lastUrl);
  const [paste, setPaste] = useState(false);
  const [text, setText] = useState('');
  const [filename, setFilename] = useState('main.py');
  // The file name follows the pasted code's language until it is edited.
  const [named, setNamed] = useState(false);
  const folder = useRef<HTMLInputElement>(null);
  const urlInput = useRef<HTMLInputElement>(null);
  const touch = useMedia(TOUCH);
  useEffect(() => {
    folder.current?.setAttribute('webkitdirectory', '');
    // On a phone, focusing would cover the choices with the keyboard.
    if (!window.matchMedia(TOUCH).matches) urlInput.current?.focus();
  }, []);
  return (
    <div className="scrim" onMouseDown={e => { if (e.target === e.currentTarget && canClose) onClose(); }}>
      <section className="open-dialog" role="dialog" aria-modal="true" aria-labelledby="open-title">
        <header>
          <div>
            <h1 id="open-title">Open a codebase</h1>
            <p>Code is analyzed in your browser with Rust/WebAssembly. Nothing is uploaded or executed.</p>
          </div>
          {canClose && <button className="icon" aria-label="Close" onClick={onClose}><X size={16} /></button>}
        </header>
        <form className="open-row" onSubmit={e => { e.preventDefault(); if (url.trim()) onOpen({ kind: 'github', url }); }}>
          <Link2 size={17} />
          <input ref={urlInput} value={url} onChange={e => setUrl(e.target.value)} placeholder="github.com/owner/repo" aria-label="GitHub repository" spellCheck={false} />
          <button className="primary" type="submit" disabled={!url.trim()}>Open</button>
        </form>
        <div className="open-options">
          <button onClick={() => onOpen({ kind: 'example' })}>
            <Sparkles size={17} />
            <span><strong>nanoGPT example</strong><small>karpathy/nanoGPT, bundled</small></span>
          </button>
          <button onClick={() => folder.current?.click()}>
            <FolderOpen size={17} />
            <span><strong>Local folder…</strong><small>{touch ? 'or files on this device' : 'or drop a folder anywhere'}</small></span>
          </button>
          <button onClick={() => setPaste(p => !p)} aria-expanded={paste}>
            <FileCode size={17} />
            <span><strong>Paste code</strong><small>a single file</small></span>
          </button>
        </div>
        {paste && (
          <div className="paste">
            <input value={filename} onChange={e => { setFilename(e.target.value); setNamed(true); }} aria-label="Filename" spellCheck={false} />
            <textarea value={text} onChange={e => { setText(e.target.value); if (!named) setFilename(guessFilename(e.target.value)); }} placeholder="Paste Python, JavaScript, TypeScript, Rust or Go here…" spellCheck={false} aria-label="Code" />
            <button className="primary" disabled={!text.trim()} onClick={() => onOpen({ kind: 'paste', filename, text })}>Visualize</button>
          </div>
        )}
        <input ref={folder} type="file" multiple hidden onChange={e => { if (e.target.files?.length) onOpen({ kind: 'folder', files: e.target.files }); e.target.value = ''; }} />
      </section>
    </div>
  );
}
