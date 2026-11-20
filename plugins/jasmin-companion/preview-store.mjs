import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const kinds = new Set(['client', 'combined', 'cpu', 'tx']);
const digest = /^[a-f0-9]{64}$/;
const publicFile = /^(?:wykresy\/)?[a-z0-9][a-z0-9.-]*\.(?:html|css|js|png)$/;
const batchSize = 4;

function checksum(buffer) { return createHash('sha256').update(buffer).digest('hex'); }

export class PreviewStore {
  #editions = new Map();
  #pending = new Map();
  #closing;

  // The caller supplies the public allowlist for this report kind and a reader
  // bound to the originating MCP analysis. No path from MCP is opened locally.
  async capture(kind, id, names, readAsset) {
    if (this.#closing) throw new Error('Preview store is closed.');
    if (!kinds.has(kind) || typeof id !== 'string' || !digest.test(id) ||
        !Array.isArray(names) || !names.length ||
        names.some(name => typeof name !== 'string' || !publicFile.test(name)))
      throw new Error('Invalid public preview asset list.');
    const key = kind + '/' + id;
    const files = [...new Set(names)].sort();
    const existing = this.#editions.get(key) || this.#pending.get(key);
    if (existing) {
      if (existing.files.length !== files.length || files.some((name, i) => name !== existing.files[i]))
        throw new Error('The asset list changed for an existing preview.');
      await existing.task;
      return;
    }
    const task = this.#captureFiles(key, files, readAsset);
    this.#pending.set(key, { files, task });
    try { await task; }
    finally { this.#pending.delete(key); }
  }

  async #captureFiles(key, files, readAsset) {
    const directory = await mkdtemp(join(tmpdir(), 'jasmin-companion-preview-'));
    const assets = new Map();
    try {
      for (let start = 0; start < files.length; start += batchSize) {
        // Settle the entire batch before cleaning up a failed capture.
        const results = await Promise.allSettled(files.slice(start, start + batchSize).map(async (name, i) => {
          const file = await readAsset(name);
          if (!Buffer.isBuffer(file?.buffer) || checksum(file.buffer) !== file.sha256)
            throw new Error('Preview asset checksum mismatch.');
          const path = join(directory, String(start + i));
          await writeFile(path, file.buffer, { flag: 'wx', mode: 0o600 });
          assets.set(name, { path, sha256: file.sha256 });
        }));
        const failure = results.find(result => result.status === 'rejected');
        if (failure) throw failure.reason;
      }
      // Readers can only find an edition after every advertised file is ready.
      this.#editions.set(key, { directory, files, assets });
    } catch (error) {
      await rm(directory, { recursive: true, force: true });
      throw error;
    }
  }

  has(kind, id, name) {
    return !this.#closing && (this.#editions.get(kind + '/' + id)?.assets.has(name) ?? false);
  }

  async read(kind, id, name) {
    const asset = this.#editions.get(kind + '/' + id)?.assets.get(name);
    if (!asset || this.#closing) return null;
    const buffer = await readFile(asset.path);
    if (checksum(buffer) !== asset.sha256) throw new Error('Stored preview asset checksum mismatch.');
    return { buffer, sha256: asset.sha256 };
  }

  close() {
    this.#closing ??= this.#cleanup();
    return this.#closing;
  }

  async #cleanup() {
    await Promise.allSettled([...this.#pending.values()].map(entry => entry.task));
    await Promise.all([...this.#editions.values()].map(({ directory }) => rm(directory, { recursive: true, force: true })));
    this.#editions.clear();
  }
}
